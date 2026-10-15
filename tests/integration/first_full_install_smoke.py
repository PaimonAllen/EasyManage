#!/usr/bin/env python3
"""First Full install with private root/filesystem and mocked systemd only.

Requires the same outer bwrap isolation as the other shell smoke tests. Never
run the installer against production system paths or real service managers.
"""
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
with (root/'calls').open('a') as log: log.write(' '.join(args)+'\\n')
case=(root/'case').read_text()
if 'show' in args: print('loaded' if case=='existing-unit' else 'not-found')
if 'start' in args and case=='start-failure': sys.exit(1)
sys.exit(0)
'''

CHECK = '''import hashlib,os,pathlib,subprocess,sys
case=sys.argv[1]
stage=pathlib.Path(('/root' if case=='valid-root' else '/home/tester')+'/.easymanage/staging/agent.ABC123xy')
stage.mkdir(parents=True)
node='11111111-2222-4333-8444-555555555555'
url='https://platform.example:8443'
values={'EASYMANAGE_SERVER_URL':url,'EASYMANAGE_NODE_ID':node,'EASYMANAGE_ENROLLMENT_TOKEN':'a'*64,
 'EASYMANAGE_CA_CERTIFICATE':'/home/tester/.easymanage/config/ca.crt',
 'EASYMANAGE_AGENT_STATE_DB':'/home/tester/.easymanage/state/state.db'}
(stage/'agent.env').write_text(''.join(key+'='+value+'\\n' for key,value in values.items()))
(stage/'easymanage-agent').write_bytes(b'fixture-binary-not-executed')
(stage/'ca.crt').write_bytes(b'fixture-ca-not-executed')
digest=lambda name:hashlib.sha256((stage/name).read_bytes()).hexdigest()
arguments=[str(stage),digest('easymanage-agent'),digest('ca.crt'),digest('agent.env'),node,url]
if case=='bad-hash': arguments[1]='0'*64
if case=='existing-state':
 state=pathlib.Path('/var/lib/easymanage-agent');state.mkdir();(state/'state.db').write_text('existing-identity')
if case=='source-symlink':
 (stage/'ca.crt').rename(stage/'source-ca');(stage/'ca.crt').symlink_to(stage/'source-ca')
if case=='unsafe-system-directory': pathlib.Path('/usr/local/sbin').chmod(0o777)
result=subprocess.run(['/bin/bash','/tmp/fixture/install.sh',*arguments],capture_output=True,timeout=40)
success=case in ('valid','valid-root')
assert (result.returncode==0)==success,(case,result.returncode,result.stderr.decode()[-500:])
unit=pathlib.Path('/etc/systemd/system/easymanage-agent.service')
env=pathlib.Path('/etc/easymanage/agent.env')
if success:
 assert unit.exists() and 'User=root' in unit.read_text()
 installed=dict(line.split('=',1) for line in env.read_text().splitlines())
 assert installed['EASYMANAGE_NODE_ID']==node and installed['EASYMANAGE_SERVER_URL']==url
 assert installed['EASYMANAGE_AGENT_STATE_DB']=='/var/lib/easymanage-agent/state.db'
 assert installed['EASYMANAGE_CA_CERTIFICATE']=='/etc/easymanage/tls/ca.crt'
 assert env.stat().st_mode&0o777==0o600 and env.stat().st_uid==0
 assert pathlib.Path('/var/lib/easymanage-agent').stat().st_uid==0
 assert not pathlib.Path('/home/tester/.config/systemd/user/easymanage-agent.service').exists()
else:
 assert not unit.exists() and not env.exists()
 if case=='existing-state': assert pathlib.Path('/var/lib/easymanage-agent/state.db').read_text()=='existing-identity'
 if case=='start-failure':
  assert not pathlib.Path('/var/lib/easymanage-agent').exists()
  assert list(pathlib.Path('/var/lib').glob('easymanage-first-install.*/failed/agent.env'))
calls=pathlib.Path('/tmp/fixture/calls').read_text()
assert '--user' not in calls and 'loginctl' not in calls
if case in ('existing-unit','existing-state','bad-hash','source-symlink','unsafe-system-directory'): assert 'start easymanage-agent.service' not in calls
print('PASS isolated first Full install: '+case,flush=True)
'''

with tempfile.TemporaryDirectory(dir="/tmp", prefix="first-full-test.") as temporary:
    root = Path(temporary)
    (root / "install.sh").write_bytes(Path("deploy/linux/operations/install-new-full-agent.sh").read_bytes())
    (root / "mock-systemctl").write_text(MOCK)
    (root / "mock-systemctl").chmod(0o755)
    (root / "check.py").write_text(CHECK)
    for case in ("valid", "valid-root", "bad-hash", "existing-unit", "existing-state", "source-symlink", "unsafe-system-directory", "start-failure"):
        (root / "case").write_text(case)
        (root / "calls").write_text("")
        subprocess.run(["bwrap", "--unshare-all", "--die-with-parent", "--uid", "0", "--ro-bind", "/", "/",
            "--dev", "/dev", "--proc", "/proc", "--tmpfs", "/usr", "--ro-bind", "/usr/bin", "/usr/bin",
            "--ro-bind", "/usr/lib", "/usr/lib", "--ro-bind", "/usr/lib64", "/usr/lib64",
            "--dir", "/usr/local/sbin", "--ro-bind", str(root / "mock-systemctl"), "/usr/bin/systemctl",
            "--tmpfs", "/var", "--dir", "/var/lib", "--tmpfs", "/etc", "--dir", "/etc/systemd/system",
            "--tmpfs", "/run", "--tmpfs", "/home", "--tmpfs", "/root", "--tmpfs", "/tmp",
            "--dir", "/tmp/fixture", "--bind", str(root), "/tmp/fixture",
            "/usr/bin/python3", "/tmp/fixture/check.py", case], check=True, timeout=50)
