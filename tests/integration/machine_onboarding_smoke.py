"""Disposable SSH/systemd-user fixture, invoked ONLY by isolated_smoke.py.

Uses Paramiko 3.5.1 in a temporary test venv; never a Server/Agent dependency.
SSH/SCP are real; Linux service-manager calls are mocked. The installed Agent
is real and must report to the temporary HTTPS/PG platform. No host services,
network configuration, private keys or credentials are read/changed.
"""
import hashlib
from isolated_smoke import parse_timestamp, print_isolated_log_tail, stop_process
import io
import json
import logging
import os
from pathlib import Path
import pwd
import secrets
import shlex
import signal
import socket
import subprocess
import sqlite3
import threading
import time
import urllib.error
import urllib.request
import uuid


def exercise(binary, root, environment, base_url, context, enrollment, require, restart_controller=None, redeployment=False, admin_only=False, disk_only=False, update_only=False,service_only=False,http2_only=False):
    import paramiko
    logging.getLogger("paramiko").setLevel(logging.CRITICAL)
    mounts = Path("/proc/self/mountinfo").read_text().splitlines()
    if not any(line.split()[4] == "/home" and line.split(" - ")[1].startswith("tmpfs ") for line in mounts):
        raise RuntimeError("Refusing SSH fixture without a private /home tmpfs")
    for path in ['/var','/etc/easymanage','/etc/systemd/system','/usr/local/sbin']:
        if not any(line.split()[4]==path and line.split(' - ')[1].startswith('tmpfs ') for line in mounts):
            raise RuntimeError('Refusing SSH file checks without masked production Agent paths')
    current = pwd.getpwuid(os.getuid())
    home = Path(current.pw_dir)
    if home.parent != Path("/home"):
        raise RuntimeError("SSH fixture needs the isolated /home/user layout")
    home.mkdir(mode=0o700, exist_ok=True)
    mock = root / "machine-fixture"
    mock.mkdir(mode=0o700)
    commands = mock / "bin"
    commands.mkdir()
    runtime = mock / "runtime"
    runtime.mkdir()
    systemctl = commands / "systemctl"
    systemctl.write_text('''#!/usr/bin/python3
import json, os, pathlib, subprocess, sys
runtime=pathlib.Path(os.environ["MACHINE_FIXTURE_RUNTIME"])
args=sys.argv[1:]
if "--user" not in args:
    if "show" in args: print("not-found" if "LoadState" in args else "inactive"); sys.exit(0)
    sys.exit(1)
args.remove("--user")
action=args[0]
pidfile=runtime/"agent.pid"
if action=="is-enabled": sys.exit(0 if (runtime/"enabled").exists() else 1)
if action=="is-active":
    try: os.kill(int(pidfile.read_text()),0)
    except (OSError,ValueError,FileNotFoundError): sys.exit(1)
    sys.exit(0)
if action=="enable": (runtime/"enabled").write_text("yes")
if action=="disable": (runtime/"enabled").unlink(missing_ok=True)
if action=="restart":
    base=pathlib.Path(os.environ["HOME"])/".easymanage"
    values=dict(line.split("=",1) for line in (base/"config/agent.env").read_text().splitlines() if "=" in line)
    env=os.environ|values|{"EASYMANAGE_ENV_FILE":str(base/"config/agent.env"),"RUST_LOG":"warn"}
    log=(runtime/"agent.log").open("ab")
    process=subprocess.Popen([str(base/"bin/easymanage-agent")],env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    pidfile.write_text(str(process.pid))
sys.exit(0)
'''.replace('os.environ["MACHINE_FIXTURE_RUNTIME"]', repr(str(runtime))))
    systemctl.chmod(0o700)
    (commands/"loginctl").write_text("#!/bin/sh\nprintf 'no\\n'\n")
    (commands/"loginctl").chmod(0o700)
    (commands/"crontab").write_text('''#!/usr/bin/python3
import pathlib,sys
table=pathlib.Path('TABLE_PLACEHOLDER')
if sys.argv[1:]==['-l']:
 if table.exists(): print(table.read_text(),end='');sys.exit(0)
 print('no crontab for fixture',file=sys.stderr);sys.exit(1)
if sys.argv[1:]==['-']: table.write_text(sys.stdin.read());sys.exit(0)
sys.exit(2)
'''.replace('TABLE_PLACEHOLDER',str(runtime/"crontab")))
    (commands/"crontab").chmod(0o700)
    password = secrets.token_urlsafe(24)
    key = paramiko.RSAKey.generate(2048)
    key_phrase = secrets.token_urlsafe(24)
    client_key = paramiko.RSAKey.generate(2048)
    plain_output, encrypted_output = io.StringIO(), io.StringIO()
    client_key.write_private_key(plain_output)
    client_key.write_private_key(encrypted_output, password=key_phrase)
    from cryptography.hazmat.primitives import serialization
    from cryptography.hazmat.primitives.asymmetric import ed25519
    ed_private = ed25519.Ed25519PrivateKey.generate().private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.OpenSSH,
        serialization.BestAvailableEncryption(key_phrase.encode())).decode()
    ed_key = paramiko.Ed25519Key.from_private_key(io.StringIO(ed_private),password=key_phrase)
    authorized_keys = [client_key.asbytes(),ed_key.asbytes()]
    alive = threading.Event()
    alive.set()
    failure = threading.Event()
    collision = threading.Event()
    attempts = []
    sudo_checks = []
    allow_full = threading.Event()
    stages = set()
    errors = []
    full_processes = []
    ssh_env = environment | {"HOME": str(home), "USER": current.pw_name,
        "PATH": str(commands) + ":/usr/bin:/bin", "MACHINE_FIXTURE_RUNTIME": str(runtime)}

    def serve_command(channel, invocation):
        try:
            command = invocation.decode()
            if command.startswith('/usr/bin/env -i ') and 'Bounded read-only disk analysis.' in command:
                process=subprocess.Popen(shlex.split(command),stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
                while True:
                    line=process.stdout.readline()
                    if not line:break
                    channel.sendall(line)
                process.stdin.close()
                result=process.wait(timeout=5)
            elif command.startswith("scp "):
                destination = Path(shlex.split(command)[-1])
                if destination.parent not in stages:
                    raise RuntimeError("SCP escaped its private staging directory")
                channel.sendall(b"\0")
                header = bytearray()
                while not header.endswith(b"\n"):
                    header.extend(channel.recv(1))
                    if len(header) > 1024: raise RuntimeError("SCP header limit")
                fields = header.decode().strip().split(" ", 2)
                size = int(fields[1])
                if size > 256 * 1024 * 1024: raise RuntimeError("SCP body limit")
                channel.sendall(b"\0")
                with destination.open("wb") as output:
                    while size:
                        data = channel.recv(min(size,65536))
                        if not data: raise RuntimeError("SCP disconnected")
                        output.write(data)
                        size -= len(data)
                destination.chmod(int(fields[0][1:],8))
                if channel.recv(1) != b"\0": raise RuntimeError("SCP terminator")
                channel.sendall(b"\0")
                result=0
            elif "$(getent passwd" in command and "$(uname -m)" in command:
                result = 1 if failure.is_set() else 0
                if not result: channel.sendall(f"{current.pw_name}\n{home}\n{os.uname().machine}\n{os.getuid()}\n".encode())
            elif "systemctl show-environment" in command:
                channel.sendall(f"{current.pw_name}\n{home}\n{os.uname().machine}\n{os.getuid()}\n".encode()); result=0
            elif command == "/usr/bin/sudo -k -S -p '' /usr/bin/true":
                submitted=bytearray()
                while not submitted.endswith(b"\n"):
                    data=channel.recv(1024)
                    if not data: break
                    submitted.extend(data)
                sudo_checks.append(secrets.compare_digest(submitted.rstrip(b"\n"),password.encode()))
                for index in range(len(submitted)): submitted[index]=0
                result=0 if allow_full.is_set() else 1  # No host sudo/systemd.
            elif command.startswith("/usr/bin/sudo -k -S -p '' /usr/bin/env -i ") and ('test "$(/usr/bin/id -u)" = 0' in command or '# Fixed, root-only service restart.' in command):
                # Administrator vault / restart fixture: authenticate the actual
                # channel stdin, but never run host sudo or service managers.
                submitted=bytearray()
                while not submitted.endswith(b"\n"):
                    data=channel.recv(1024)
                    if not data: break
                    submitted.extend(data)
                correct=secrets.compare_digest(submitted.rstrip(b"\n"),password.encode())
                for index in range(len(submitted)): submitted[index]=0
                result=0 if allow_full.is_set() and correct else 1
            elif command.startswith('test ! -e "$HOME/.local/share/easymanage/'):
                result=0
            elif command.startswith("python3 -c ") and ("/api/v1/health/ready" in command or "/proc/net/tcp" in command):
                completed=subprocess.run(shlex.split(command),env=ssh_env,capture_output=True,timeout=8)
                channel.sendall(completed.stdout);result=completed.returncode
            elif command.startswith("/usr/bin/python3 -I -c "):
                payload=bytearray()
                while True:
                    data=channel.recv(4096)
                    if not data: break
                    payload.extend(data)
                    if len(payload)>8192: raise RuntimeError("Fixture network payload limit")
                completed=subprocess.run(shlex.split(command),input=payload,env=ssh_env,capture_output=True,timeout=15)
                for index in range(len(payload)): payload[index]=0
                channel.sendall(completed.stdout);result=completed.returncode
            elif command.startswith("/usr/bin/python3 -c ") and "same-identity Agent repair" in command:
                completed=subprocess.run(shlex.split(command),env=ssh_env,capture_output=True,timeout=10)
                channel.sendall(completed.stdout);result=completed.returncode
            elif "test ! -e /etc/easymanage/agent.env" in command:
                if collision.is_set(): result=1
                elif allow_full.is_set():
                    # The second logical target gets its own root filesystem
                    # below. Its actual Full installer repeats all existence
                    # checks there, not in target one's shared Basic home.
                    result=0
                else:
                    # The outer namespace masks production /var and system
                    # Agent paths. Execute real file-presence checks here.
                    completed=subprocess.run(shlex.split(command),env=ssh_env,capture_output=True,timeout=10)
                    channel.sendall(completed.stdout);result=completed.returncode
            elif command.startswith('umask 077;') and 'mktemp -d ' in command and '.easymanage/staging/agent.' in command:
                stage=home/".easymanage/staging"/("agent."+secrets.token_hex(4))
                stage.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
                stage.mkdir(mode=0o700)
                stages.add(stage)
                channel.sendall((str(stage)+"\n").encode()); result=0
            elif command.startswith("/usr/bin/sudo -k -S -p '' /usr/bin/timeout"):
                # Execute the actual fixed Full installer as namespace-root,
                # sharing only the already-isolated loopback for a real Agent.
                # No sudo or production systemd is ever executed by this fixture.
                submitted=bytearray()
                while not submitted.endswith(b"\n"):
                    data=channel.recv(1024)
                    if not data: break
                    submitted.extend(data)
                if not secrets.compare_digest(submitted.rstrip(b"\n"),password.encode()): raise RuntimeError("Fixture sudo authentication failed")
                for index in range(len(submitted)): submitted[index]=0
                tokens=shlex.split(command)
                invocation=tokens[tokens.index("/usr/bin/timeout"):]
                source_stage=Path(tokens[tokens.index("--")+1])
                full=mock/"full"
                full.mkdir(mode=0o700)
                (full/"systemctl").write_text('''#!/usr/bin/python3
import os,pathlib,subprocess,sys
root=pathlib.Path('/tmp/full-fixture')
args=sys.argv[1:]
if 'show' in args: print('not-found')
if 'start' in args:
 env=os.environ|{'EASYMANAGE_ENV_FILE':'/etc/easymanage/agent.env','EASYMANAGE_ADMIN_POLICY':'require','RUST_LOG':'warn'}
 log=(root/'agent.log').open('ab')
 subprocess.Popen(['/usr/local/sbin/easymanage-agent'],env=env,stdout=log,stderr=subprocess.STDOUT)
sys.exit(0)
''')
                (full/"systemctl").chmod(0o700)
                log=(full/"installer.log").open("ab")
                runner="import pathlib,subprocess,sys,time; r=subprocess.run(sys.argv[1:]); pathlib.Path('/tmp/full-fixture/install-result').write_text(str(r.returncode)); time.sleep(120 if r.returncode==0 else 0); sys.exit(r.returncode)"
                process=subprocess.Popen(["bwrap","--unshare-user","--unshare-pid","--unshare-ipc","--unshare-uts","--die-with-parent","--uid","0",
                    "--ro-bind","/","/","--dev","/dev","--proc","/proc","--tmpfs","/usr",
                    "--ro-bind","/usr/bin","/usr/bin","--ro-bind","/usr/lib","/usr/lib","--ro-bind","/usr/lib64","/usr/lib64",
                    "--dir","/usr/local/sbin","--ro-bind",str(full/"systemctl"),"/usr/bin/systemctl",
                    "--tmpfs","/var","--dir","/var/lib","--tmpfs","/etc","--dir","/etc/systemd/system",
                    "--tmpfs","/run","--tmpfs","/home","--tmpfs","/root","--tmpfs","/tmp",
                    "--dir","/tmp/full-fixture","--bind",str(full),"/tmp/full-fixture",
                    "--ro-bind",str(source_stage),str(source_stage),
                    "/usr/bin/python3","-c",runner,*invocation],stdout=log,stderr=subprocess.STDOUT)
                full_processes.append(process)
                deadline=time.monotonic()+95
                while not (full/"install-result").exists() and process.poll() is None and time.monotonic()<deadline: time.sleep(0.1)
                if not (full/"install-result").exists(): raise RuntimeError("Full fixture installer did not finish")
                result=int((full/"install-result").read_text())
                channel.sendall(b"namespace Full installer finished\n")
                channel.send_exit_status(result);channel.shutdown_write();channel.close()
                # A real system service survives the installation SSH channel.
                # Keep namespace init alive until the fixture explicitly stops it,
                # not for a fixed eight-second race with initial inventory.
                while alive.is_set() and process.poll() is None: time.sleep(0.1)
                stop_process(process);log.close()
                return
            elif "/bin/bash" in command and "install-user-agent.sh" in command:
                log=(runtime/"installer.log").open("ab")
                if "EASYMANAGE_UPDATE_" in command:
                    completed=subprocess.run(["/bin/sh","-c",command],env=ssh_env,capture_output=True,timeout=100)
                    log.write(completed.stdout);log.write(completed.stderr);log.close()
                    channel.sendall(completed.stdout+completed.stderr)
                    # Simulate a detached descendant retaining stdout: a verified
                    # receipt must complete the update without waiting for EOF.
                    while alive.is_set() and not channel.closed:time.sleep(.1)
                    return
                result=subprocess.run(shlex.split(command),env=ssh_env,stdout=log,stderr=subprocess.STDOUT,timeout=100).returncode
                log.close()
                channel.sendall(b"fixture installer finished\n")
            elif command.startswith("/usr/bin/timeout ") and "agent-user-control.py" in command:
                completed=subprocess.run(shlex.split(command),env=ssh_env,capture_output=True,timeout=45)
                channel.sendall(completed.stdout);result=completed.returncode
            elif "agent-user-control.py" in command and command.endswith(" describe"):
                completed=subprocess.run(shlex.split(command),env=ssh_env,capture_output=True,timeout=5)
                channel.sendall(completed.stdout); result=completed.returncode
            elif command.startswith("cd ") and "unlink SHA256SUMS" in command:
                result=subprocess.run(["/bin/sh","-c",command],env=ssh_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5).returncode
            else:
                raise RuntimeError("Unexpected fixture SSH command")
            channel.send_exit_status(result)
            channel.shutdown_write()
        except Exception as error:
            errors.append(type(error).__name__ + (":" + str(error) if isinstance(error,RuntimeError) else ""))
            channel.send_exit_status(1)
        finally:
            channel.close()

    allow_forward = threading.Event()
    allow_forward.set()
    max_connections = [0]
    def bridge(channel, connection):
        import select
        try:
            channel.settimeout(1); connection.settimeout(1)
            while alive.is_set():
                readable,_,_=select.select([channel,connection],[],[],0.2)
                for source in readable:
                    data=source.recv(16384)
                    if not data: return
                    destination=connection if source is channel else channel
                    destination.sendall(data)
        except (OSError,EOFError): pass
        finally: channel.close();connection.close()

    class Server(paramiko.ServerInterface):
        def __init__(self, transport):
            self.transport=transport
            self.forwards={}
        def check_port_forward_request(self, address, port):
            if not allow_forward.is_set() or address!="127.0.0.1": return False
            listener=socket.socket()
            listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
            try: listener.bind((address,port));listener.listen(8);listener.settimeout(0.2)
            except OSError: listener.close();return False
            actual=listener.getsockname()[1]
            self.forwards[actual]=listener
            def accept_forward():
                while alive.is_set() and self.transport.is_active() and actual in self.forwards:
                    try: connection,origin=listener.accept()
                    except socket.timeout: continue
                    except OSError: break
                    try:
                        channel=self.transport.open_forwarded_tcpip_channel(origin,(address,actual))
                        threading.Thread(target=bridge,args=(channel,connection),daemon=True).start()
                    except Exception: connection.close();break
                listener.close()
            threading.Thread(target=accept_forward,daemon=True).start()
            return actual
        def check_cancel_port_forward_request(self, address, port):
            listener=self.forwards.pop(port,None)
            if listener: listener.close()
        def check_auth_password(self, username, provided):
            attempts.append(username)  # Never save/log passwords, even fake ones.
            return paramiko.AUTH_SUCCESSFUL if username == current.pw_name and secrets.compare_digest(provided,password) else paramiko.AUTH_FAILED
        def check_auth_publickey(self, username, provided):
            attempts.append(username)  # Public key comparison only; no credential logging.
            return paramiko.AUTH_SUCCESSFUL if username==current.pw_name and any(secrets.compare_digest(provided.asbytes(),allowed) for allowed in authorized_keys) else paramiko.AUTH_FAILED
        def get_allowed_auths(self, username): return "password,publickey"
        def check_channel_request(self, kind, chanid): return paramiko.OPEN_SUCCEEDED if kind=="session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED
        def check_channel_exec_request(self, channel, invocation):
            threading.Thread(target=serve_command,args=(channel,invocation),daemon=True).start()
            return True

    listener=socket.socket()
    listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
    listener.bind(("127.0.0.1",2222));listener.listen();listener.settimeout(0.2)
    transports=[]
    def accept():
        while alive.is_set():
            try: client,_=listener.accept()
            except socket.timeout: continue
            except OSError: break
            transport=paramiko.Transport(client)
            transports.append(transport)
            max_connections[0]=max(max_connections[0],sum(t.is_active() for t in transports)+1)
            transport.add_server_key(key)
            try: transport.start_server(server=Server(transport))
            except Exception: transport.close()
    threading.Thread(target=accept,daemon=True).start()
    import http.cookiejar
    cookies=http.cookiejar.CookieJar()
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context),urllib.request.HTTPCookieProcessor(cookies))
    def request(path,body=None,headers=None):
        data=None if body is None else json.dumps(body).encode()
        headers={"Content-Type":"application/json"}|(headers or {})
        if body is not None: headers["Origin"]=base_url
        try:
            with opener.open(urllib.request.Request(base_url+path,data=data,headers=headers),timeout=30) as response:
                return response.status,json.load(response)
        except urllib.error.HTTPError as error:
            return error.code,json.load(error)
    def wait_job(created):
        deadline=time.monotonic()+120
        while time.monotonic()<deadline:
            status,job=request("/api/v1/machines/jobs/"+created["job_id"],headers={"Authorization":"Bearer "+created["job_key"]})
            if status!=200: raise AssertionError("Task polling failed")
            if job["status"]!="running": return job
            time.sleep(0.25)
        raise AssertionError("Onboarding fixture timed out")
    def begin_redeployment(node_id,body):
        deadline=time.monotonic()+75
        while True:
            status,result=request(f'/api/v1/machines/{node_id}/redeploy',body)
            if status!=429 or time.monotonic()>=deadline:return status,result
            time.sleep(5)  # Respect per-node operation limits from earlier tests.
    def exercise_admin(full_created, http2_only=False):
        admin_path=f'/api/v1/nodes/{full_created["node_id"]}/admin-credentials'
        def admin_request(body):
            deadline=time.monotonic()+70
            while True:
                status,value=request(admin_path,body)
                if status!=429 or time.monotonic()>deadline: return status,value
                time.sleep(5)
        require(request(admin_path)[1]=={'available':False},'ordinary transport authorization cannot authorize administrator credential reuse')
        admin_key={'username':current.pw_name,'private_key':ed_private,'passphrase':key_phrase,'sudo_password':password}
        admin_file=root/'ssh-vault'/('admin-'+full_created['node_id']+'.enc')
        if not http2_only:
            require(admin_request(admin_key|{'passphrase':'wrong-fixture-key-phrase'})[0]==401,
                    'administrator private-key authentication failure is rejected before vault persistence')
            require(not admin_file.exists(),'failed administrator authentication does not create encrypted credential record')
        require(admin_request({'username':current.pw_name,'password':password})[0]==200,
                'first administrator password authentication verifies root authorization and saves encrypted credentials')
        require(request(admin_path)[1]=={'available':True},'only authorized browser sees saved administrator credentials as usable')
        if http2_only:
            # Browsers can encode each cookie as its own HTTP/2 field. Exercise the
            # real TLS/HTTP2 server, with this machine's admin cookie after another
            # cookie; fixture capabilities travel through stdin, never argv/logs.
            import ssl
            ca_file=mock/'http2-ca.crt'
            ca_file.write_text(''.join(ssl.DER_cert_to_PEM_cert(value) for value in context.get_ca_certs(binary_form=True)))
            cookie_name='em_admin_'+full_created['node_id'].replace('-','')
            cookie_value=next(cookie.value for cookie in cookies if cookie.name==cookie_name)
            def http2_request(authorized=True,renew=False):
                curl_config='header = "Cookie: unrelated=fixture"\n'
                if authorized:curl_config+='header = "Cookie: '+cookie_name+'='+cookie_value+'"\n'
                if renew:curl_config+='header = "Content-Type: application/json"\nheader = "Origin: '+base_url+'"\ndata = "{\\"use_saved\\":true}"\n'
                result=subprocess.run(['/usr/bin/curl','--http2','--noproxy','*','--silent','--show-error','--max-time','15',
                    '--cacert',str(ca_file),'--config','-','--write-out','\n%{http_version}\n%{http_code}',base_url+admin_path],
                    input=curl_config.encode(),capture_output=True,timeout=17)
                body,version,code=result.stdout.rsplit(b'\n',2) if result.stdout.count(b'\n')>=2 else (b'{}',b'',b'000')
                require(result.returncode==0 and version==b'2','administrator regression uses real TLS-verified HTTP/2')
                return int(code),json.loads(body)
            code,body=http2_request()
            require(code==200 and body=={'available':True},
                    'real HTTP/2 accepts split browser cookies without losing the selected machine administrator authorization')
            require(http2_request(False)==(200,{'available':False}),'unrelated HTTP/2 cookies do not authorize another browser')
            code,body=http2_request(renew=True)
            require(code==200 and body['available'],'HTTP/2 renews encrypted administrator authorization without new passwords or keys')
            require(request(admin_path)[1]=={'available':True},'HTTP/2 renewal retains existing HTTP/1.1 browser authorization')
            restart_controller()
            require(http2_request()==(200,{'available':True}),'HTTP/2 split-cookie administrator authorization survives platform restart')
            return
        no_cookie=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
        with no_cookie.open(base_url+admin_path,timeout=5) as response:
            require(json.load(response)=={'available':False},'administrator status never authorizes a different browser or reveals credentials')
        require(admin_request(admin_key|{'sudo_password':'wrong-fixture-sudo'})[0]!=200,
                'valid SSH key without successful sudo cannot replace administrator credentials')
        require(request(admin_path)[1]['available'],'failed sudo preserves previously verified administrator authorization')
        require(admin_request({'use_saved':True,'username':current.pw_name})[0]==400,'mixed saved and supplied administrator credentials are refused')
        require(admin_request(admin_key)[0]==200,'encrypted OpenSSH key, passphrase and distinct sudo credential are accepted by administrator vault')
        for secret in (password.encode(),key_phrase.encode(),ed_private.encode()):
            require(secret not in admin_file.read_bytes(),'administrator ciphertext contains no plaintext fixture secret')
        admin_cookie=next(c.value for c in cookies if c.name=='em_admin_'+full_created['node_id'].replace('-',''))
        require(admin_request({'use_saved':True})[0]==200,'saved administrator credentials verify and renew without resubmitting passwords or keys')
        require(next(c.value for c in cookies if c.name=='em_admin_'+full_created['node_id'].replace('-',''))==admin_cookie,'sliding authorization renewal does not rotate active browser capability')
        restart_path=f'/api/v1/nodes/{full_created["node_id"]}/agent/restart'
        deadline=time.monotonic()+70
        while True:
            status,restarted=request(restart_path,{'use_saved':True})
            if status!=429 or time.monotonic()>deadline:break
            time.sleep(5)
        if status!=200:
            print('DIAGNOSTIC saved Full restart:',status,restarted.get('message'), 'mock errors:',errors,flush=True)
            _,listing=request('/api/v1/nodes')
            observed=next(node for node in listing['nodes'] if node['id']==full_created['node_id'])
            print('DIAGNOSTIC Full heartbeat:',observed['last_seen'],observed['metrics'].get('agent_control'),flush=True)
        require(status==200,'Full Agent restart resolves encrypted administrator credentials without explicit SSH password or key')
        if restart_controller is not None:
            restart_controller()
            require(request(admin_path)[1]['available'],'platform restart preserves encrypted administrator credentials and scoped browser authorization')
        deadline=time.monotonic()+70
        while True:
            try:
                with opener.open(urllib.request.Request(base_url+admin_path,method='DELETE',headers={'Origin':base_url}),timeout=10) as response:
                    require(json.load(response)=={'available':False},'administrator credentials can be explicitly cleared')
                break
            except urllib.error.HTTPError as error:
                if error.code!=429 or time.monotonic()>deadline:raise
                time.sleep(1)  # Platform readiness precedes the background SSH restore lease being released.
        require(not admin_file.exists() and (root/'ssh-vault'/(full_created['node_id']+'.enc')).is_file(),
                'clearing administrator credentials leaves ordinary SSH transport credentials intact')
    try:
        if admin_only or http2_only:
            allow_full.set()
            _,probe=request('/api/v1/machines/probe',{'host':'10.222.0.10','port':2222})
            payload={'host':'10.222.0.10','port':2222,'display_name':'Admin SSH fixture','server_url':base_url,
                'fingerprint':probe['fingerprint'],'username':current.pw_name,'auth_method':'private_key',
                'private_key':ed_private,'passphrase':key_phrase,'sudo_password':password,'deployment_mode':'full','connection_mode':'ssh'}
            status,created=request('/api/v1/machines',payload)
            require(status==200 and wait_job(created)['status']=='success','administrator fixture starts with real Full SSH installation and heartbeat')
            exercise_admin(created,http2_only)
            return
        _,probe=request("/api/v1/machines/probe",{"host":"10.222.0.9","port":2222})
        require("fingerprint" in probe and not attempts,"onboarding fingerprint probe never sends SSH credentials")
        payload={"host":"10.222.0.9","port":2222,"display_name":"SSH fixture","server_url":base_url,
            "fingerprint":probe["fingerprint"],"username":current.pw_name,"password":password}
        status,created=request("/api/v1/machines",payload|{"fingerprint":"changed"})
        if status!=200: print("DIAGNOSTIC onboarding creation:",status,created.get("message"),flush=True)
        require(status==200 and wait_job(created)["status"]=="failed" and not attempts,
                "changed SSH fingerprint blocks authentication and remote installation")
        key_base={k:v for k,v in payload.items() if k!="password"}|{"auth_method":"private_key"}
        key_payload=key_base|{"private_key":ed_private,"passphrase":key_phrase}
        status,created=request("/api/v1/machines",key_payload|{"fingerprint":"changed"})
        require(status==200 and wait_job(created)["status"]=="failed" and not attempts,
                "key authentication also verifies fingerprint before any SSH identity proof")
        failure.set()  # Stop after authentication; these cases must not install a second identity.
        for label,credentials in [("legacy password",payload),
            ("unencrypted PEM RSA key",key_base|{"private_key":plain_output.getvalue()}),
            ("encrypted PEM RSA key",key_base|{"private_key":encrypted_output.getvalue(),"passphrase":key_phrase})]:
            status,created=request("/api/v1/machines",credentials)
            result=wait_job(created) if status==200 else None
            require(result and result["status"]=="failed" and any("SSH 认证成功" in e["message"] for e in result["events"]),
                    label+" authenticates using real SSH before the mocked prerequisite failure")
        for label,credentials in [("wrong key passphrase",key_payload|{"passphrase":"wrong-fixture-phrase"})]:
            status,created=request("/api/v1/machines",credentials)
            result=wait_job(created) if status==200 else None
            require(result and result["status"]=="failed" and not any("SSH 认证成功" in e["message"] for e in result["events"]),
                    label+" is rejected without uploading or persisting a key")
            require(key_phrase not in json.dumps(result) and ed_private not in json.dumps(result),"SSH key errors do not expose key material or passphrases")
        failure.clear()
        require(request("/api/v1/machines",payload|{"username":"root"})[0]==400,
                "Basic mode rejects root before any deployment")
        for credentials in [payload,key_payload|{"sudo_password":password}]:
            status,created=request("/api/v1/machines",credentials|{"deployment_mode":"full"})
            result=wait_job(created) if status==200 else None
            require(result and result["status"]=="failed" and not any("已上传" in e["message"] for e in result["events"]),
                    "Full first deployment verifies sudo before files/identity; refusal cannot become Basic fallback")
        require(sudo_checks==[True,True],"SSH password reuse and explicit key-login sudo password travel only through SSH stdin")
        status,created=request("/api/v1/machines",key_payload|{"server_url":"https://10.222.0.254:58443","connection_mode":"auto"})
        require(status==200,"encrypted OpenSSH Ed25519 onboarding returns a bounded background deployment task")
        require(request("/api/v1/machines/jobs/"+created["job_id"])[0]==401,"deployment logs require their per-job capability")
        job=wait_job(created)
        if job["status"]!="success":
            # Workflow messages are public/sanitized. Never print installer/Agent logs.
            print("DIAGNOSTIC onboarding:", [event["message"] for event in job["events"]], "fixture errors:",errors,flush=True)
        require(job["status"]=="success","real SCP, checksum installer and actual Basic heartbeat complete onboarding")
        launch=json.loads((home/".easymanage/config/agent-launch.json").read_text())
        require(launch["backend"]=="compatibility","Linger=no / missing user manager installs a real Basic compatibility supervisor")
        require(any("兼容守护" in event["message"] for event in job["events"]),"deployment log reports compatibility backend and actual autostart limitations")
        # Auto onboarding used an actually unreachable HTTPS URL; all real
        # heartbeats above therefore traversed the reverse SSH HTTPS stream.
        node_id=created["node_id"]
        ssh_auth={"username":current.pw_name,"auth_method":"private_key","private_key":ed_private,"passphrase":key_phrase}
        _,machines=request("/api/v1/machines")
        persistent=next(machine for machine in machines["machines"] if machine["node_id"]==node_id)
        require(persistent["connection_mode"]=="ssh" and persistent["ssh_transport"]["connections"]==2,"control and reverse HTTPS use exactly two long TCP SSH connections")
        if service_only:
            from agent_service_http_smoke import exercise as exercise_service
            exercise_service(request,wait_job,begin_redeployment,node_id,current.pw_name,home,ssh_env,base_url,context,restart_controller,require,transports,max_connections)
            return
        if disk_only:
            dials=len(transports)
            scan_path=f'/api/v1/nodes/{node_id}/disk-space'
            status,created_scan=request(scan_path,{'mode':'basic','path':'/tmp/work/tests/integration/fixtures','seconds':30})
            require(status==200,'SSH-only disk scan starts on the authenticated persistent control session')
            deadline=time.monotonic()+25
            while time.monotonic()<deadline:
                _,snapshot=request(scan_path+'/'+created_scan['id'])
                if snapshot['status']!='running':break
                time.sleep(.2)
            require(snapshot['status']=='complete' and snapshot['result']['uid']==os.getuid(),
                    'real SSH fixed scanner returns the original ordinary-user identity and directory tree')
            require(len(transports)==dials,'disk scanning opens a channel without creating any new SSH connections')
            current_machine=next(m for m in request('/api/v1/machines')[1]['machines'] if m['node_id']==node_id)
            require(current_machine['ssh_transport']['connections']==2,'reverse HTTPS reporting remains connected during disk analysis')
            return
        net=request('/api/v1/platform-network')[1]
        net_groups={g['group']:g for g in net['server']['groups']}
        ssh_started=net_groups['ssh_connect']['started']
        require(ssh_started>=2 and 2<=net_groups['ssh_connect']['open_connections']<=3
                and net_groups['ssh_exec']['started']>0
                and net_groups['ssh_scp']['submitted_bytes']>=binary.stat().st_size
                and net_groups['ssh_tunnel']['received_bytes']>0 and net_groups['ssh_tunnel']['submitted_bytes']>0,
                'real SSH handshakes, exec/SCP data and reverse HTTPS bytes are counted separately without extra connections')
        require(persistent["credentials_saved"] and persistent["saved_credentials_usable"],"successful verified onboarding encrypts credentials and authorizes only this browser")
        require(persistent['deployment_mode']=='basic' and not persistent['redeploy_available'],"online machines expose their original mode but cannot be redeployed")
        redeploy_request={'confirmation':node_id,'username':current.pw_name,'use_saved':True,'connection_mode':'ssh','server_url':base_url}
        require(request(f'/api/v1/machines/{node_id}/redeploy',redeploy_request)[0]==400,"redeploy refuses a machine with recent actual heartbeats before any upload")
        no_cookie=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
        try:
            no_cookie.open(urllib.request.Request(base_url+f"/api/v1/machines/{node_id}/ssh",data=json.dumps({"action":"disconnect","username":current.pw_name,"use_saved":True}).encode(),headers={"Content-Type":"application/json","Origin":base_url}),timeout=5)
        except urllib.error.HTTPError as error:
            require(error.code==401,"knowing node UUID and same origin is insufficient to use saved credentials from an unauthorized browser")
        else: raise AssertionError("Saved credential capability was not enforced")
        dial_count=len(transports)
        for _ in range(3):
            require(request("/api/v1/machines/probe",{"host":"10.222.0.9","port":2222})[0]==200,"repeated fingerprint reads reuse the authenticated control handshake")
            request("/api/v1/platform-health")
        require(len(transports)==dial_count,"status/fingerprint/health inspection does not initiate more SSH connections")
        current_net=request('/api/v1/platform-network')[1]
        require(next(g for g in current_net['server']['groups'] if g['group']=='ssh_connect')['started']==ssh_started,
                'reused channels and network inspection never masquerade as new SSH connection attempts')
        monitoring_path=f'/api/v1/machines/{node_id}/monitoring'
        require(request(monitoring_path,{'confirmation':node_id,'paused':True,'expected_revision':0})[0]==200,
                'SSH-only node accepts monitoring pause without new authentication or installation')
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            paused_machine=next(m for m in request('/api/v1/machines')[1]['machines'] if m['node_id']==node_id)
            if paused_machine['monitoring_applied_revision']==1:break
            time.sleep(.25)
        else:raise AssertionError('SSH-only Agent did not acknowledge monitoring pause')
        frozen=next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==node_id)
        time.sleep(12)
        retained=next(m for m in request('/api/v1/machines')[1]['machines'] if m['node_id']==node_id)
        require(retained['ssh_transport']['connections']==2 and len(transports)==dial_count
                and next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==node_id)['metrics']==frozen['metrics'],
                'pause stops SSH Agent metrics while retaining the same two established SSH sessions')
        require(request(monitoring_path,{'confirmation':node_id,'paused':False,'expected_revision':1})[0]==200,
                'page resume reuses the original SSH-only Agent identity')
        deadline=time.monotonic()+75
        while time.monotonic()<deadline:
            resumed_node=next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==node_id)
            if resumed_node['online'] and resumed_node['metrics'].get('agent_control')=={'revision':2,'paused':False}:break
            time.sleep(.5)
        else:
            observed=next(m for m in request('/api/v1/machines')[1]['machines'] if m['node_id']==node_id)
            print('DIAGNOSTIC SSH pause:',{k:observed.get(k) for k in ['monitoring_revision','monitoring_applied_revision','control_seen_at','ssh_transport']},flush=True)
            log_path=home/'.easymanage/logs/agent.log'
            if log_path.is_file():
                with log_path.open() as paused_log: print_isolated_log_tail(paused_log,'SSH pause Agent',[enrollment,password,key_phrase,ed_private])
            raise AssertionError('SSH-only Agent did not resume through its minute control poll')
        require(len(transports)==dial_count and resumed_node['metrics']['privilege']==frozen['metrics']['privilege'],
                'SSH-only minute control resumes real metrics without additional SSH handshakes or privilege changes')
        for transport in transports: transport.close()
        after_disconnect=time.time()
        deadline=time.monotonic()+110
        while time.monotonic()<deadline:
            _,snapshot=request("/api/v1/nodes")
            resumed=next((node for node in snapshot["nodes"] if node["id"]==created["node_id"]),None)
            if resumed and parse_timestamp(resumed["last_seen"]).timestamp()>after_disconnect: break
            time.sleep(0.25)
        if not resumed or parse_timestamp(resumed["last_seen"]).timestamp()<=after_disconnect:
            _,machines=request("/api/v1/machines")
            print("DIAGNOSTIC SSH recovery:",next((m.get("ssh_transport") for m in machines["machines"] if m["node_id"]==node_id),None),"fixture errors:",errors,flush=True)
        require(resumed and parse_timestamp(resumed["last_seen"]).timestamp()>after_disconnect,"tunnel and control reconnect after one minute and resume real Agent reports after SSH disconnect")
        _,machines=request("/api/v1/machines")
        persistent=next(machine for machine in machines["machines"] if machine["node_id"]==node_id)
        require(persistent["ssh_transport"]["connections"]<=3 and persistent["ssh_transport"]["reconnects"]>=1,"recovery remains within the three-SSH budget")
        dial_count=len(transports)
        restart_status,restart_result=request(f"/api/v1/nodes/{node_id}/agent/restart",{})
        if restart_status!=200: print("DIAGNOSTIC pooled restart:",restart_status,restart_result,errors,flush=True)
        require(restart_status==200,"Basic restart executes over the persistent control connection")
        require(len(transports)==dial_count,"Basic restart does not launch a new SSH process/handshake")
        candidate=root/"controller-home/.easymanage/releases/current/target/release/easymanage-agent"
        candidate.write_bytes(candidate.read_bytes()+b'\0isolated-new-release')
        registry_path=root/"managed-targets.json"
        overlay=json.loads(registry_path.read_text())
        for target in overlay["targets"]:
            if target["node_id"]==node_id: target["release_sha256"]=hashlib.sha256(candidate.read_bytes()).hexdigest()
        registry_path.write_text(json.dumps(overlay));registry_path.chmod(0o600)
        update_started=time.monotonic()
        update_status,update_result=request(f"/api/v1/nodes/{node_id}/agent/update",{})
        if update_status!=200: print("DIAGNOSTIC pooled update:",update_status,update_result,errors,flush=True)
        require(update_status==200 and len(transports)==dial_count,"real binary update/SCP and Basic restart reuse control without a new SSH handshake")
        require(time.monotonic()-update_started<20,"hash-verified update receipt completes even when the remote output stream stays open")
        if update_only:
            require(max_connections[0]<=3,"framed update keeps the established SSH connection budget")
            return
        require(max_connections[0]<=3,"all observed SSH handshakes including probes stay within the connection cap")
        saved_auth={"username":current.pw_name,"use_saved":True}
        require(request(f"/api/v1/machines/{node_id}/ssh",saved_auth|{"action":"disconnect"})[0]==200,"authorized browser disconnects without resending its private key and pauses automatic restore")
        time.sleep(0.5)
        allow_forward.clear()
        require(request(f"/api/v1/machines/{node_id}/ssh",saved_auth|{"action":"connect"})[0]!=200,"disabled SSH forwarding is reported without modifying sshd or bypassing policy")
        allow_forward.set()
        require(request(f"/api/v1/machines/{node_id}/ssh",saved_auth|{"action":"connect"})[0]==200,"same machine reconnects from encrypted saved key after forwarding is explicitly allowed")
        require((runtime/"crontab").read_text().count("# BEGIN EasyManage Agent compatibility")==1,"compatibility autostart updates only its managed user-cron block")
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            old_agent=json.loads((home/".easymanage/state/compat-process.json").read_text()).get("agent_pid")
            if old_agent: break
            time.sleep(0.1)
        require(bool(old_agent),"pending network/restart operation starts a real Agent child before crash recovery test")
        os.kill(old_agent,signal.SIGKILL)  # Exact disposable Agent, never a host PID.
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            current_agent=json.loads((home/".easymanage/state/compat-process.json").read_text()).get("agent_pid")
            if current_agent and current_agent!=old_agent: break
            time.sleep(0.25)
        require(current_agent!=old_agent,"unprivileged supervisor automatically restarts a crashed Agent without systemd/linger")
        require(all(stage.parent==home/".easymanage/staging" for stage in stages),"all Basic deployment staging stays inside the SSH user's ~/.easymanage")
        values=dict(line.split("=",1) for line in (home/".easymanage/config/agent.env").read_text().splitlines() if "=" in line)
        require(values["EASYMANAGE_ENROLLMENT_TOKEN"]!=enrollment,"target receives only a scoped token, never the platform enrollment secret")
        require(all(secret not in json.dumps(job) for secret in [password,key_phrase,ed_private,values["EASYMANAGE_ENROLLMENT_TOKEN"]]),"deployment progress/logs exclude passwords, private keys, passphrases and enrollment secrets")
        sensitive=[password.encode(),key_phrase.encode(),ed_private.encode(),plain_output.getvalue().encode(),encrypted_output.getvalue().encode()]
        installed=home/".easymanage"
        files=[p for p in installed.rglob("*") if p.is_file() and p.stat().st_size<2*1024*1024]
        files.extend(root.glob("*.log"))
        files.extend(root.glob("*.json"))
        files.extend(runtime.glob("*.log"))
        files.extend((root/"ssh-vault").glob("*.enc"))
        for file in files:
            content=file.read_bytes()
            require(not any(secret in content for secret in sensitive),"credentials absent from persisted fixture file: "+file.name)
        node_id=created["node_id"]
        _,listing=request("/api/v1/nodes")
        require(any(node["id"]==node_id for node in listing["nodes"]),"new machine appears in the overview after confirmed enrollment")
        real_node=next(node for node in listing["nodes"] if node["id"]==node_id)
        scoped=values["EASYMANAGE_ENROLLMENT_TOKEN"]
        enroll={"protocol_version":1,"requested_node_id":node_id,"agent_version":"0.5.0","inventory":real_node}
        require(request("/api/v1/agent/enroll",enroll,{"x-easymanage-enrollment-token":scoped})[0]==401,"single-node onboarding token cannot be reused after enrollment")
        with sqlite3.connect("file:"+str(home/".easymanage/state/state.db")+"?mode=ro",uri=True) as db:
            identity_before=dict(db.execute("SELECT key,value FROM metadata WHERE key IN ('node_id','agent_token')"))
        change={"username":current.pw_name,"password":password,"auth_scope":"target","confirmation":node_id,"display_name":"Renamed SSH fixture","note":"fixture note"}
        require(request(f"/api/v1/machines/{node_id}/manage",change|{"action":"delete","password":"wrong"})[0]==401,"soft deletion requires successful SSH authentication")
        require(request(f"/api/v1/machines/{node_id}/manage",change|{"action":"edit"})[0]==200,"SSH-authenticated machine edit changes label without changing UUID")
        require(request(f"/api/v1/machines/{node_id}/manage",change|{"action":"delete"})[0]==200,"SSH-authenticated deletion is soft, not a remote uninstall")
        _,listing=request("/api/v1/nodes")
        require(not any(node["id"]==node_id for node in listing["nodes"]),"soft-deleted node is hidden from current monitoring")
        require((home/".easymanage/state/state.db").is_file(),"soft deletion retains the target's SQLite identity and Agent files")
        require(request("/api/v1/agent/enroll",enroll,{"x-easymanage-enrollment-token":enrollment})[0]==401,"even the historical global enroll secret cannot resurrect a soft-deleted UUID")
        _,history=request(f"/api/v1/nodes/{node_id}/history")
        require(bool(history["points"]),"soft deletion preserves historical metrics")
        _,machines=request("/api/v1/machines")
        require(any(machine["node_id"]==node_id and machine["deleted_at"] for machine in machines["machines"]),"archived machine remains listed with its original ID")
        require(request(f"/api/v1/machines/{node_id}/manage",change|{"action":"restore"})[0]==200,"restore re-enables the original machine credential without reinstalling")
        # Soft deletion pauses auto-recovery but retains encrypted credentials.
        # Explicit restoration/reattach must keep original SQLite identity.
        time.sleep(1)
        deadline=time.monotonic()+75
        while True:
            restore_status,restore_result=request(f"/api/v1/machines/{node_id}/ssh",saved_auth|{"action":"connect"})
            if restore_status!=429 or time.monotonic()>=deadline: break
            time.sleep(5)  # Respect the existing HTTP/SSH-operation rate limits.
        if restore_status!=200: print("DIAGNOSTIC restored SSH:",restore_status,restore_result,errors,flush=True)
        require(restore_status==200,"restored SSH node reconnects explicitly using original authentication")
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            _,listing=request("/api/v1/nodes")
            if any(node["id"]==node_id and node["online"] and node["hostname"]=="Renamed SSH fixture" for node in listing["nodes"]): break
            time.sleep(0.25)
        require(any(node["id"]==node_id and node["hostname"]=="Renamed SSH fixture" for node in listing["nodes"]),"restored machine keeps its display name and original UUID")
        with sqlite3.connect("file:"+str(home/".easymanage/state/state.db")+"?mode=ro",uri=True) as db:
            identity_after=dict(db.execute("SELECT key,value FROM metadata WHERE key IN ('node_id','agent_token')"))
        require(identity_after==identity_before,"restore preserves the exact SQLite identity and original Agent credential")
        require(max_connections[0]<=3,"credentialed management and recovery never exceed three observed SSH connections")
        if redeployment:
            _,before_nodes=request('/api/v1/nodes')
            _,before_history=request(f'/api/v1/nodes/{node_id}/history')
            preserved_points={point['collected_at'] for point in before_history['points']}
            require(preserved_points,"redeployment fixture starts with real persisted history")
            subprocess.run(['/usr/bin/python3',str(home/'.easymanage/bin/agent-user-control.py'),'stop'],env=ssh_env,check=True,capture_output=True,timeout=25)
            deadline=time.monotonic()+70
            while time.monotonic()<deadline:
                _,machines=request('/api/v1/machines')
                inactive=next(machine for machine in machines['machines'] if machine['node_id']==node_id)
                if inactive['redeploy_available']:break
                time.sleep(0.5)
            require(inactive['redeploy_available'],"offline original node becomes eligible after the actual online window expires")
            # Reject another identity, but explicitly repair this original one.
            original_env=(home/'.easymanage/config/agent.env').read_bytes()
            database=home/'.easymanage/state/state.db'
            with sqlite3.connect(database) as db:
                db.execute("UPDATE metadata SET value=? WHERE key='node_id'",(str(uuid.uuid4()),))
            original_db=database.read_bytes()
            status,blocked=begin_redeployment(node_id,redeploy_request)
            blocked_job=wait_job(blocked) if status==200 else None
            if not blocked_job or blocked_job['status']!='failed' or any('已上传' in event['message'] for event in blocked_job['events']):
                print('DIAGNOSTIC residual redeploy:',status,blocked.get('message') if status!=200 else [event['message'] for event in blocked_job['events']],errors,flush=True)
            require(blocked_job and blocked_job['status']=='failed' and not any('已上传' in event['message'] for event in blocked_job['events']),"another node's SQLite identity refuses overwrite before upload")
            require((home/'.easymanage/config/agent.env').read_bytes()==original_env and database.read_bytes()==original_db,"refused cross-node overwrite does not rewrite config or SQLite bytes")
            _,machines=request('/api/v1/machines')
            require(next(machine for machine in machines['machines'] if machine['node_id']==node_id)['saved_credentials_usable'],"failed preflight does not revoke the authorized browser's saved-credential capability")
            with sqlite3.connect(database) as db:
                db.execute("UPDATE metadata SET value=? WHERE key='node_id'",(node_id,))
                db.execute("INSERT OR REPLACE INTO metadata(key,value) VALUES('repair-history-sentinel','preserve-me')")
                invalid_retained_token=secrets.token_hex(32)
                db.execute("UPDATE metadata SET value=? WHERE key='agent_token'",(invalid_retained_token,))
            status,repaired=begin_redeployment(node_id,redeploy_request)
            repaired_job=wait_job(repaired) if status==200 else None
            if not repaired_job or repaired_job['status']!='success':print('DIAGNOSTIC overwrite repair:',status,[event['message'] for event in (repaired_job or {}).get('events',[])],errors,flush=True)
            require(repaired_job and repaired_job['status']=='success','same-UUID stopped installation can be overwritten without uninstalling its database')
            with sqlite3.connect(database) as db:
                repaired_identity=dict(db.execute("SELECT key,value FROM metadata WHERE key IN ('node_id','agent_token','repair-history-sentinel')"))
                require(repaired_identity['node_id']==node_id and repaired_identity['agent_token'] not in (invalid_retained_token,identity_before['agent_token'])
                        and repaired_identity['repair-history-sentinel']=='preserve-me','overwrite retains UUID and local history while recovering an invalid retained Agent token')
            backups=list((home/'.easymanage').glob('install-backup.*/state.db'))
            require(backups,'overwrite creates a consistent private SQLite recovery backup')
            require(preserved_points<={point['collected_at'] for point in request(f'/api/v1/nodes/{node_id}/history')[1]['points']},'overwrite retains every pre-existing platform history point')
            subprocess.run(['/usr/bin/python3',str(home/'.easymanage/bin/agent-user-control.py'),'stop'],env=ssh_env,check=True,capture_output=True,timeout=25)
            deadline=time.monotonic()+70
            while time.monotonic()<deadline:
                inactive=next(machine for machine in request('/api/v1/machines')[1]['machines'] if machine['node_id']==node_id)
                if inactive['redeploy_available']:break
                time.sleep(.5)
            require(inactive['redeploy_available'],'repaired fixture can be stopped for the separate missing-install redeployment test')
            original_db=database.read_bytes()
            archive=home/('.easymanage-uninstalled-'+secrets.token_hex(4))
            (home/'.easymanage').rename(archive)  # Only this isolated fixture.
            require(request(f'/api/v1/machines/{node_id}/redeploy',redeploy_request|{'confirmation':'wrong'})[0]==400,"redeployment requires explicit confirmation of the original UUID")
            require(request(f'/api/v1/machines/{node_id}/redeploy',redeploy_request|{'host':'10.222.0.10'})[0]==400,"redeployment cannot replace the trusted target address through request fields")
            dial_count=len(transports)
            status,recreated=begin_redeployment(node_id,redeploy_request)
            if status!=200:print('DIAGNOSTIC recreate request:',status,recreated.get('message'),flush=True)
            require(status==200 and recreated['node_id']==node_id,"redeployment creates a new scoped task for the original UUID, not another node")
            require(request('/api/v1/machines/jobs/'+created['job_id'],headers={'Authorization':'Bearer '+created['job_key']})[0]==401,"replacing a completed deployment task rotates its log capability")
            require(request(f'/api/v1/machines/{node_id}/redeploy',redeploy_request)[0]==429,"a running redeploy task cannot be started twice")
            recreated_job=wait_job(recreated)
            if recreated_job['status']!='success':print('DIAGNOSTIC redeployment:',[event['message'] for event in recreated_job['events']],errors,flush=True)
            require(recreated_job['status']=='success',"uninstalled Basic node redeploys over real SCP/checksums and requires a new real Agent heartbeat")
            require(len(transports)==dial_count and max_connections[0]<=3,"redeployment reuses the existing control and reverse HTTPS sessions without another SSH handshake")
            _,after_nodes=request('/api/v1/nodes')
            _,after_history=request(f'/api/v1/nodes/{node_id}/history')
            _,machines=request('/api/v1/machines')
            restored=next(machine for machine in machines['machines'] if machine['node_id']==node_id)
            require(len(after_nodes['nodes'])==len(before_nodes['nodes']) and restored['display_name']=='Renamed SSH fixture' and restored['note']=='fixture note',"redeploy preserves node count, original display name and management note")
            require(preserved_points<={point['collected_at'] for point in after_history['points']},"redeploy retains every existing historical metric point")
            with sqlite3.connect('file:'+str(home/'.easymanage/state/state.db')+'?mode=ro',uri=True) as db:
                new_identity=dict(db.execute("SELECT key,value FROM metadata WHERE key IN ('node_id','agent_token')"))
            require(new_identity['node_id']==identity_before['node_id'] and new_identity['agent_token']!=identity_before['agent_token'],"fresh installation reuses UUID and rotates only the lost/old Agent credential")
            old_report={'protocol_version':1,'node_id':node_id,'agent_version':'0.5.0','inventory':real_node,'metrics':real_node['metrics']}
            require(request('/api/v1/agent/report',old_report,{'x-easymanage-agent-token':identity_before['agent_token']})[0]==401,"old archived Agent token can no longer report after redeployment enrollment")
            require((archive/'state/state.db').read_bytes()==original_db,"the isolated uninstall archive is not touched by the platform")
        if restart_controller and not redeployment:
            since=time.time()
            restart_controller()
            deadline=time.monotonic()+35
            while time.monotonic()<deadline:
                _,listing=request('/api/v1/nodes')
                restored=next((node for node in listing['nodes'] if node['id']==node_id),None)
                if restored and parse_timestamp(restored['last_seen']).timestamp()>since: break
                time.sleep(0.25)
            require(restored and parse_timestamp(restored['last_seen']).timestamp()>since,"real platform restart reloads encrypted key and resumes actual Agent heartbeats without resubmitting SSH credentials")
            _,machines=request('/api/v1/machines')
            persisted=next(machine for machine in machines['machines'] if machine['node_id']==node_id)
            require(persisted['saved_credentials_usable'] and persisted['ssh_transport']['connections']==2,"browser capability and two-session transport survive platform restart")
        if not redeployment:
            # Exercise the actual production retry policy, without a fake clock.
            allow_forward.clear()
            for transport in transports: transport.close()
            failed_at=time.monotonic()
            deadline=failed_at+250
            while time.monotonic()<deadline:
                _,machines=request('/api/v1/machines')
                retry=next(machine['ssh_transport'] for machine in machines['machines'] if machine['node_id']==node_id)
                if retry['status']=='retry_exhausted': break
                time.sleep(0.5)
            require(retry['status']=='retry_exhausted' and retry['retry_attempt']==3 and retry['connections']==0,"three real one-minute reconnect failures stop and release every SSH connection")
            require(time.monotonic()-failed_at>=180,"retry policy never starts three reconnect attempts sooner than three minutes")
            dial_count=len(transports)
            time.sleep(2)
            require(len(transports)==dial_count,"retry exhaustion stops further SSH handshakes until explicit manual reconnect")
            allow_forward.set()
            require(request(f'/api/v1/machines/{node_id}/ssh',saved_auth|{'action':'connect'})[0]==200,"manual retry after exhaustion reuses encrypted credentials without a password prompt")
        collision.set()
        status,blocked=request("/api/v1/machines",payload|{"host":"10.222.0.10"})
        blocked_job=wait_job(blocked) if status==200 else None
        require(status==200 and blocked_job["status"]=="failed" and not any("已上传" in event["message"] for event in blocked_job["events"]),
                "existing or inaccessible system Agent identity is refused before any file upload")
        collision.clear()
        failure.set()
        status,failed=request("/api/v1/machines",payload|{"host":"10.222.0.10"})
        require(status==200 and wait_job(failed)["status"]=="failed","other invalid prerequisites still fail without claiming deployment success")
        failure.clear()
        allow_full.set()
        status,full_created=request("/api/v1/machines",key_payload|{"host":"10.222.0.10","deployment_mode":"full","sudo_password":password,"connection_mode":"ssh"})
        full_job=wait_job(full_created) if status==200 else None
        if not full_job or full_job["status"]!="success":
            print("DIAGNOSTIC Full onboarding:",status,[event["message"] for event in (full_job or {}).get("events",[])],"fixture errors:",errors,flush=True)
            for path in (mock/"full/agent.log",mock/"full/installer.log"):
                if path.is_file():
                    with path.open("rb") as log:
                        print_isolated_log_tail(log,"private Full fixture "+path.name,[password,key_phrase,ed_private,enrollment])
        require(full_job and full_job["status"]=="success","first Full SSH deployment runs fixed installer and confirms real namespace-root Agent heartbeat without Basic/linger")
        _,listing=request("/api/v1/nodes")
        full_node=next(node for node in listing["nodes"] if node["id"]==full_created["node_id"])
        require(full_node["metrics"]["privilege"]["uid"]==0 and full_node["metrics"]["privilege"]["mode"]=="full",
                "Full deployment success requires the selected privilege and UID 0, not a Basic heartbeat")
        require(all(secret not in json.dumps(full_job) for secret in (password,key_phrase,ed_private)),"Full deployment logs exclude SSH keys, sudo passwords and private-key passphrases")
        exercise_admin(full_created)
    finally:
        helper=home/".easymanage/bin/agent-user-control.py"
        if helper.is_file(): subprocess.run(["/usr/bin/python3",str(helper),"stop"],env=ssh_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=25)
        alive.clear();listener.close()
        for process in full_processes: stop_process(process)
        for transport in transports: transport.close()
        pidfile=runtime/"agent.pid"
        if pidfile.exists():
            try: os.kill(int(pidfile.read_text()),signal.SIGTERM)
            except ProcessLookupError: pass
