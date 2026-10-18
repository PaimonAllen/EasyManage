#!/usr/bin/env python3
"""Root Full SSH update/network scripts in private root namespaces, mocked systemd."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

assert len(sys.argv) == 3
for kind, original in zip(("net", "mnt"), sys.argv[1:]):
    assert os.readlink(f"/proc/self/ns/{kind}") != original

MOCK = '''#!/usr/bin/python3
import pathlib,sys
root=pathlib.Path('/tmp/fixture')
args=sys.argv[1:]
if 'show' in args: print('root');sys.exit(0)
if 'restart' in args and (root/'case').read_text()=='start-failure': sys.exit(1)
sys.exit(0)
'''
CHECK = '''import hashlib,json,pathlib,subprocess,sys
root=pathlib.Path('/tmp/fixture');case=(root/'case').read_text()
stage=pathlib.Path('/home/tester/.easymanage/staging/agent.ABC123xy');stage.mkdir(parents=True)
stage.joinpath('easymanage-agent').write_bytes(b'new fixture release')
binary=pathlib.Path('/usr/local/sbin/easymanage-agent');binary.write_bytes(b'original Full binary');binary.chmod(0o755)
config=pathlib.Path('/etc/easymanage/agent.env');node='11111111-2222-4333-8444-555555555555'
config.write_text('EASYMANAGE_NODE_ID='+node+'\\nEASYMANAGE_SERVER_URL=https://old.example:8443\\nEASYMANAGE_ENROLLMENT_TOKEN='+'a'*64+'\\n');config.chmod(0o600)
ca=pathlib.Path('/etc/easymanage/tls/ca.crt');ca.write_bytes(b'public fixture CA');ca.chmod(0o600)
state=pathlib.Path('/var/lib/easymanage-agent/state.db');state.write_bytes(b'private unchanged Full identity')
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
result=subprocess.run(['/bin/bash','-x','/tmp/fixture/update.sh',str(stage),node if case!='wrong-node' else '22222222-2222-4333-8444-555555555555',sha(stage/'easymanage-agent') if case!='bad-hash' else '0'*64,sha(ca)],capture_output=True,timeout=30)
assert (result.returncode==0)==(case=='valid'),(case,result.returncode,result.stderr.decode()[-1600:])
assert state.read_bytes()==b'private unchanged Full identity'
assert binary.read_bytes()==(b'new fixture release' if case=='valid' else b'original Full binary')
if case=='valid':
 result=subprocess.run(['/usr/bin/python3','/tmp/fixture/network.py','/home/tester/.easymanage',node,'https://127.0.0.1:18443',sha(ca),'full'],input=json.dumps({'enrollment_token':None}).encode(),capture_output=True,timeout=10)
 assert result.returncode==0,result.stderr.decode()
 assert 'https://127.0.0.1:18443' in config.read_text()
 assert 'EASYMANAGE_ENROLLMENT_TOKEN='+'a'*64 in config.read_text()
 assert not pathlib.Path('/home/tester/.easymanage/config/agent.env').exists()
 assert state.read_bytes()==b'private unchanged Full identity'
 assert config.stat().st_mode&0o777==0o600
'''

for case in ("valid", "bad-hash", "wrong-node", "start-failure"):
    with tempfile.TemporaryDirectory(prefix="full-ssh-update.", dir="/tmp") as temporary:
        fixture = Path(temporary)
        (fixture / "systemctl").write_text(MOCK)
        (fixture / "systemctl").chmod(0o700)
        (fixture / "case").write_text(case)
        (fixture / "update.sh").write_bytes(Path("deploy/linux/operations/update-full-over-ssh.sh").read_bytes())
        (fixture / "network.py").write_bytes(Path("deploy/linux/operations/configure-ssh-network.py").read_bytes())
        command = ["bwrap", "--unshare-user", "--unshare-pid", "--die-with-parent", "--uid", "0",
            "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/usr",
            "--ro-bind", "/usr/bin", "/usr/bin", "--ro-bind", "/usr/lib", "/usr/lib", "--ro-bind", "/usr/lib64", "/usr/lib64",
            "--dir", "/usr/local/sbin", "--ro-bind", str(fixture / "systemctl"), "/usr/bin/systemctl",
            "--tmpfs", "/etc", "--dir", "/etc/easymanage/tls", "--tmpfs", "/var", "--dir", "/var/lib/easymanage-agent",
            "--tmpfs", "/home", "--tmpfs", "/tmp", "--bind", str(fixture), "/tmp/fixture", "/usr/bin/python3", "-c", CHECK]
        result = subprocess.run(command, capture_output=True, timeout=45)
        assert result.returncode == 0, result.stderr.decode()[-1000:]
        print("PASS isolated Full SSH update/network: " + case, flush=True)
