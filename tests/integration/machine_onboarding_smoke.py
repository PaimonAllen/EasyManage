"""Disposable SSH/systemd-user fixture, invoked ONLY by isolated_smoke.py.

Uses Paramiko 3.5.1 in a temporary test venv; never a Server/Agent dependency.
SSH/SCP are real; Linux service-manager calls are mocked. The installed Agent
is real and must report to the temporary HTTPS/PG platform. No host services,
network configuration, private keys or credentials are read/changed.
"""
import hashlib
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


def exercise(binary, root, environment, base_url, context, enrollment, require):
    import paramiko
    logging.getLogger("paramiko").setLevel(logging.CRITICAL)
    mounts = Path("/proc/self/mountinfo").read_text().splitlines()
    if not any(line.split()[4] == "/home" and line.split(" - ")[1].startswith("tmpfs ") for line in mounts):
        raise RuntimeError("Refusing SSH fixture without a private /home tmpfs")
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
    if "show" in args: print("inactive"); sys.exit(0)
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
''')
    systemctl.chmod(0o700)
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
    ssh_env = environment | {"HOME": str(home), "USER": current.pw_name,
        "PATH": str(commands) + ":/usr/bin:/bin", "MACHINE_FIXTURE_RUNTIME": str(runtime)}

    def serve_command(channel, invocation):
        try:
            command = invocation.decode()
            if command.startswith("scp "):
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
            elif "loginctl show-user" in command:
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
            elif command.startswith('test ! -e "$HOME/.local/share/easymanage/'):
                result=0
            elif "test ! -e /etc/easymanage/agent.env" in command:
                result=1 if collision.is_set() else 0
            elif 'mktemp -d "$HOME/.easymanage/staging/agent.' in command:
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
                runner="import subprocess,sys,time; r=subprocess.run(sys.argv[1:]); time.sleep(8 if r.returncode==0 else 0); sys.exit(r.returncode)"
                result=subprocess.run(["bwrap","--unshare-user","--unshare-pid","--unshare-ipc","--unshare-uts","--die-with-parent","--uid","0",
                    "--ro-bind","/","/","--dev","/dev","--proc","/proc","--tmpfs","/usr",
                    "--ro-bind","/usr/bin","/usr/bin","--ro-bind","/usr/lib","/usr/lib","--ro-bind","/usr/lib64","/usr/lib64",
                    "--dir","/usr/local/sbin","--ro-bind",str(full/"systemctl"),"/usr/bin/systemctl",
                    "--tmpfs","/var","--dir","/var/lib","--tmpfs","/etc","--dir","/etc/systemd/system",
                    "--tmpfs","/run","--tmpfs","/home","--tmpfs","/root","--tmpfs","/tmp",
                    "--dir","/tmp/full-fixture","--bind",str(full),"/tmp/full-fixture",
                    "--ro-bind",str(source_stage),str(source_stage),
                    "/usr/bin/python3","-c",runner,*invocation],stdout=log,stderr=subprocess.STDOUT,timeout=100).returncode
                log.close()
                channel.sendall(b"namespace Full installer finished\n")
            elif "/bin/bash" in command and "install-user-agent.sh" in command:
                log=(runtime/"installer.log").open("ab")
                result=subprocess.run(shlex.split(command),env=ssh_env,stdout=log,stderr=subprocess.STDOUT,timeout=100).returncode
                log.close()
                channel.sendall(b"fixture installer finished\n")
            elif command.startswith("cd ") and "unlink SHA256SUMS" in command:
                result=subprocess.run(["/bin/sh","-c",command],env=ssh_env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5).returncode
            else:
                raise RuntimeError("Unexpected fixture SSH command")
            channel.send_exit_status(result)
            channel.shutdown_write()
        except Exception as error:
            errors.append(type(error).__name__)
            channel.send_exit_status(1)
        finally:
            channel.close()

    class Server(paramiko.ServerInterface):
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
            transport.add_server_key(key)
            try: transport.start_server(server=Server())
            except Exception: transport.close()
    threading.Thread(target=accept,daemon=True).start()
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
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
    try:
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
        status,created=request("/api/v1/machines",key_payload)
        require(status==200,"encrypted OpenSSH Ed25519 onboarding returns a bounded background deployment task")
        require(request("/api/v1/machines/jobs/"+created["job_id"])[0]==401,"deployment logs require their per-job capability")
        job=wait_job(created)
        if job["status"]!="success":
            # Workflow messages are public/sanitized. Never print installer/Agent logs.
            print("DIAGNOSTIC onboarding:", [event["message"] for event in job["events"]], "fixture errors:",errors,flush=True)
        require(job["status"]=="success","real SCP, checksum installer and actual Basic heartbeat complete onboarding")
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
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            _,listing=request("/api/v1/nodes")
            if any(node["id"]==node_id and node["online"] and node["hostname"]=="Renamed SSH fixture" for node in listing["nodes"]): break
            time.sleep(0.25)
        require(any(node["id"]==node_id and node["hostname"]=="Renamed SSH fixture" for node in listing["nodes"]),"restored machine keeps its display name and original UUID")
        with sqlite3.connect("file:"+str(home/".easymanage/state/state.db")+"?mode=ro",uri=True) as db:
            identity_after=dict(db.execute("SELECT key,value FROM metadata WHERE key IN ('node_id','agent_token')"))
        require(identity_after==identity_before,"restore preserves the exact SQLite identity and original Agent credential")
        collision.set()
        status,blocked=request("/api/v1/machines",payload|{"host":"10.222.0.10"})
        blocked_job=wait_job(blocked) if status==200 else None
        require(status==200 and blocked_job["status"]=="failed" and not any("已上传" in event["message"] for event in blocked_job["events"]),
                "existing or inaccessible system Agent identity is refused before any file upload")
        collision.clear()
        failure.set()
        status,failed=request("/api/v1/machines",payload|{"host":"10.222.0.10"})
        require(status==200 and wait_job(failed)["status"]=="failed","missing user-service/linger prerequisites fail instead of reporting deployment success")
        failure.clear()
        allow_full.set()
        status,full_created=request("/api/v1/machines",key_payload|{"host":"10.222.0.10","deployment_mode":"full","sudo_password":password})
        full_job=wait_job(full_created) if status==200 else None
        if not full_job or full_job["status"]!="success":
            print("DIAGNOSTIC Full onboarding:",status,[event["message"] for event in (full_job or {}).get("events",[])],"fixture errors:",errors,flush=True)
        require(full_job and full_job["status"]=="success","first Full SSH deployment runs fixed installer and confirms real namespace-root Agent heartbeat without Basic/linger")
        _,listing=request("/api/v1/nodes")
        full_node=next(node for node in listing["nodes"] if node["id"]==full_created["node_id"])
        require(full_node["metrics"]["privilege"]["uid"]==0 and full_node["metrics"]["privilege"]["mode"]=="full",
                "Full deployment success requires the selected privilege and UID 0, not a Basic heartbeat")
        require(all(secret not in json.dumps(full_job) for secret in (password,key_phrase,ed_private)),"Full deployment logs exclude SSH keys, sudo passwords and private-key passphrases")
    finally:
        alive.clear();listener.close()
        for transport in transports: transport.close()
        pidfile=runtime/"agent.pid"
        if pidfile.exists():
            try: os.kill(int(pidfile.read_text()),signal.SIGTERM)
            except ProcessLookupError: pass
