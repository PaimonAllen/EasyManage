#!/usr/bin/env python3
"""Mock service managers in private namespaces; never contact production systemd."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

for namespace, original in zip(("net", "mnt"), sys.argv[1:]):
    assert os.readlink(f"/proc/self/ns/{namespace}") != original
assert len(sys.argv) == 3

MOCK = '''#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root = Path('/tmp/fixture')
case = (root/'case').read_text()
name = Path(sys.argv[0]).name
args = sys.argv[1:]
with (root/'calls').open('a') as f: f.write(json.dumps([name,args])+'\\n')
if name == 'id': print('1001'); sys.exit(0)
if name == 'sleep': sys.exit(0)
if name == 'runuser':
    command = args[args.index('--')+1:]; os.execv(command[0], command)
if name == 'systemd-run':
    command = args[args.index('/usr/sbin/runuser'):]; os.execv(command[0], command)
if name == 'systemctl':
    user = '--user' in args
    if user: args.remove('--user')
    command = args[0]
    if command == 'show':
        field = args[args.index('-p')+1]
        value = {'LoadState':'loaded', 'User':'root', 'ActiveState':'failed' if case == 'full-failed' else 'inactive',
            'ExecStart':'{ path=/home/tester/.local/share/easymanage/bin/easymanage-server ; argv[]=fixture ; }'}[field]
        if case == 'wrong-user' and field == 'User': value = 'other'
        if case == 'wrong-platform' and field == 'ExecStart': value = 'foreign binary'
        print(value); sys.exit(0)
    if command == 'is-enabled': sys.exit(0 if (user and case in ('basic','restart-failed')) or (not user and case in ('full-enabled','wrong-user')) else 1)
    if command == 'restart' and case == 'restart-failed': sys.exit(3)
    sys.exit(0)
sys.exit(1)
'''

with tempfile.TemporaryDirectory(dir='/tmp', prefix='force-restart-test.') as temporary:
    root = Path(temporary)
    mocks = []
    for name, absolute in [('id','/usr/bin/id'), ('sleep','/usr/bin/sleep'), ('systemctl','/usr/bin/systemctl'),
                           ('runuser','/usr/sbin/runuser'), ('systemd-run','/usr/bin/systemd-run')]:
        path = root/name
        path.write_text(MOCK)
        path.chmod(0o755)
        mocks.extend(['--ro-bind',str(path),absolute])
    script = Path('deploy/linux/operations/force-restart-service.sh').resolve()
    for case, action, success in [('full-failed','agent',True),('full-enabled','agent',True),
        ('basic','agent',True),('both-disabled','agent',False),('restart-failed','agent',False),('wrong-user','agent',False),
        ('platform','platform',True),('wrong-platform','platform',False),('invalid-action','reboot',False)]:
        (root/'case').write_text(case)
        (root/'calls').write_text('')
        result = subprocess.run(['bwrap','--unshare-all','--uid','0','--ro-bind','/','/','--dev','/dev','--proc','/proc',
            '--tmpfs','/run','--tmpfs','/home','--tmpfs','/tmp','--dir','/tmp/fixture','--bind',str(root),'/tmp/fixture',
            '--ro-bind',str(script),'/tmp/script',*mocks,'/bin/bash','/tmp/script',action,'tester'], capture_output=True, timeout=10)
        assert (result.returncode == 0) == success, (case,result.returncode,result.stderr.decode())
        calls = [json.loads(line) for line in (root/'calls').read_text().splitlines()]
        kills = [args for name,args in calls if name=='systemctl' and 'kill' in args]
        if success:
            assert len(kills)==1 and '--signal=SIGKILL' in kills[0] and '--kill-who=all' in kills[0]
            assert kills[0][-1] == ('easymanage-server.service' if action=='platform' else 'easymanage-agent.service')
            assert ('--user' in kills[0]) == (case in ('basic','platform'))
        if case=='platform':
            assert any(name=='systemd-run' and '--on-active=3s' in args for name,args in calls)
        if case in ('wrong-user','wrong-platform','invalid-action','both-disabled'): assert not kills
        print('PASS mocked force restart',case,flush=True)
