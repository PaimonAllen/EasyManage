#!/usr/bin/env python3
"""Promotion sequencing in private root directories with mocked managers/users."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

assert len(sys.argv)==3
for name,original in zip(("net","mnt"),sys.argv[1:]):
    assert os.readlink(f"/proc/self/ns/{name}")!=original

MOCK='''#!/usr/bin/python3
import json,pathlib,sys
root=pathlib.Path('/tmp/fixture')
name=pathlib.Path(sys.argv[0]).name
args=sys.argv[1:]
with (root/'calls').open('a') as log: log.write(json.dumps([name,args])+'\\n')
if name=='getent': print('tester:x:1001:1001::/home/tester:/bin/bash');sys.exit(0)
if name=='id': print('1001');sys.exit(0)
if name=='runuser':
 assert args[:2]==['-u','tester']
 if 'agent-user-control.py' in ' '.join(args):
  assert '/usr/bin/python3' in args
  if args[-1]=='stop': (root/'compat-stopped').touch()
  if args[-1]=='start': (root/'compat-restored').touch()
  sys.exit(0)
 assert 'systemctl' in args
 if 'show' in args: print('not-found');sys.exit(0)
 sys.exit(1)
if name=='systemctl':
 action=args[0]
 if action in ('is-active','is-enabled'): sys.exit(0 if (root/'full-started').exists() else 1)
 if action=='restart':
  assert (root/'compat-stopped').exists()
  if (root/'case').read_text()=='start-failure': sys.exit(1)
  (root/'full-started').touch()
 if action=='stop': (root/'full-started').unlink(missing_ok=True)
 sys.exit(0)
sys.exit(2)
'''
CHECK='''import hashlib,json,pathlib,subprocess,sys
root=pathlib.Path('/tmp/fixture')
base=pathlib.Path('/home/tester/.easymanage')
for name in ('bin','config','state'): (base/name).mkdir(parents=True,exist_ok=True)
(base/'bin/easymanage-agent').write_bytes(b'fixture-release-not-executed')
(base/'bin/agent-user-control.py').write_text('must never execute as root')
(base/'config/ca.crt').write_bytes(b'fixture-ca')
node='11111111-2222-4333-8444-555555555555'
url='https://platform.example:8443'
(base/'config/agent.env').write_text('EASYMANAGE_SERVER_URL='+url+'\\nEASYMANAGE_NODE_ID='+node+'\\nEASYMANAGE_ENROLLMENT_TOKEN='+'a'*64+'\\n')
(base/'config/agent-launch.json').write_text(json.dumps({'backend':'compatibility'}))
(base/'state/compat.enabled').touch()
state=base/'state/state.db';state.write_bytes(b'original-user-identity')
case=(root/'case').read_text()
if case in ('large-state','oversized-state'):
 with state.open('r+b') as stream:stream.truncate(172236800 if case=='large-state' else 268435457)
def sha(path):
 digest=hashlib.sha256()
 with path.open('rb') as stream:
  for chunk in iter(lambda:stream.read(1024*1024),b''):digest.update(chunk)
 return digest.hexdigest()
original_size=state.stat().st_size
original_hash=sha(state) if case!='oversized-state' else None
args=[str(base),'tester',sha(base/'bin/easymanage-agent'),node,url,sha(base/'config/ca.crt')]
result=subprocess.run(['/bin/bash','/tmp/fixture/promote.sh',*args],capture_output=True,timeout=45)
success=case in ('valid','large-state')
assert (result.returncode==0)==success,(result.returncode,result.stderr.decode()[-500:])
if case=='oversized-state':
 assert not (root/'compat-stopped').exists()
 assert state.stat().st_size==original_size
 assert '256 MiB limit' in result.stderr.decode()
 print('PASS state above 256 MiB refused before service changes',flush=True)
 sys.exit(0)
assert (root/'compat-stopped').exists()
assert state.stat().st_size==original_size and sha(state)==original_hash
if success:
 full_state=pathlib.Path('/var/lib/easymanage-agent/state.db')
 assert full_state.stat().st_size==original_size and sha(full_state)==original_hash
 assert 'User=root' in pathlib.Path('/etc/systemd/system/easymanage-agent.service').read_text()
 assert not (root/'compat-restored').exists()
else:
 assert (root/'compat-restored').exists()
 assert not pathlib.Path('/var/lib/easymanage-agent/state.db').exists()
calls=[json.loads(line) for line in (root/'calls').read_text().splitlines()]
assert any(name=='runuser' and 'agent-user-control.py' in ' '.join(args) and args[-1]=='stop' for name,args in calls)
print('PASS isolated compatibility promotion sequencing: '+(root/'case').read_text(),flush=True)
'''

with tempfile.TemporaryDirectory(dir="/tmp",prefix="compat-promotion-test.") as temporary:
    root=Path(temporary)
    (root/"promote.sh").write_bytes(Path("deploy/linux/operations/promote-agent.sh").read_bytes())
    (root/"check.py").write_text(CHECK)
    binds=[]
    for name,target in (("systemctl","/usr/bin/systemctl"),("runuser","/usr/sbin/runuser"),("getent","/usr/bin/getent"),("id","/usr/bin/id")):
        mock=root/name
        mock.write_text(MOCK)
        mock.chmod(0o755)
        binds.extend(["--ro-bind",str(mock),target])
    for case in ("valid","large-state","oversized-state","start-failure"):
        (root/"case").write_text(case)
        (root/"calls").write_text("")
        (root/"compat-stopped").unlink(missing_ok=True)
        (root/"compat-restored").unlink(missing_ok=True)
        (root/"full-started").unlink(missing_ok=True)
        subprocess.run(["bwrap","--unshare-all","--die-with-parent","--uid","0","--ro-bind","/","/","--dev","/dev","--proc","/proc",
            "--tmpfs","/usr","--ro-bind","/usr/bin","/usr/bin","--ro-bind","/usr/sbin","/usr/sbin",
            "--ro-bind","/usr/lib","/usr/lib","--ro-bind","/usr/lib64","/usr/lib64","--dir","/usr/local/sbin",*binds,
            "--tmpfs","/var","--dir","/var/lib","--tmpfs","/etc","--dir","/etc/systemd/system",
            "--tmpfs","/run","--tmpfs","/home","--tmpfs","/root","--tmpfs","/tmp",
            "--dir","/tmp/fixture","--bind",str(root),"/tmp/fixture","/usr/bin/python3","/tmp/fixture/check.py"],check=True,timeout=50)
