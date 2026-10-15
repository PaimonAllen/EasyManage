#!/usr/bin/env python3
"""Exercise the fixed scheduling shell with private mocked system managers only."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

assert len(sys.argv) == 3
for namespace, original in zip(("net", "mnt"), sys.argv[1:]):
    assert os.readlink(f"/proc/self/ns/{namespace}") != original

MOCK = '''#!/usr/bin/python3
import json, os, sys
from pathlib import Path
root=Path('/tmp/fixture')
case=(root/'case').read_text()
name=Path(sys.argv[0]).name
args=sys.argv[1:]
with (root/'calls').open('a') as f: f.write(json.dumps([name,args])+'\\n')
if name=='id': print('1001'); sys.exit(0)
if name=='runuser':
    command=args[args.index('--')+1:]; os.execv(command[0],command)
if name=='systemd-run': sys.exit(1 if case=='already-scheduled' else 0)
if name=='systemctl':
    if 'show' in args:
        field=args[args.index('-p')+1]
        print('loaded' if field=='LoadState' else ('foreign' if case=='wrong-unit' else '{ path=/home/tester/.easymanage/bin/easymanage-server ; argv[]=fixture ; }'))
    sys.exit(0)
sys.exit(1)
'''
CODE = '''import json,sys
from pathlib import Path
root=Path('/tmp/fixture')
with (root/'checks').open('a') as f: f.write(json.dumps(sys.argv[1:])+'\\n')
if (root/'case').read_text()=='bad-candidate': sys.exit(3)
'''

source = Path('server/src/platform_update.rs').read_text()
scheduler = re.search(r'const SCHEDULER: &str = r#"(.*?)"#;', source, re.S).group(1)
with tempfile.TemporaryDirectory(dir='/tmp', prefix='platform-schedule-test.') as temporary:
    root=Path(temporary)
    (root/'scheduler').write_text(scheduler)
    mocks=[]
    for name, absolute in [('id','/usr/bin/id'),('systemctl','/usr/bin/systemctl'),
                           ('runuser','/usr/sbin/runuser'),('systemd-run','/usr/bin/systemd-run')]:
        path=root/name
        path.write_text(MOCK)
        path.chmod(0o755)
        mocks.extend(['--ro-bind',str(path),absolute])
    for case, owner, success in [('valid','tester',True),('wrong-unit','tester',False),
                                ('bad-candidate','tester',False),('already-scheduled','tester',False),('root-owner','root',False)]:
        (root/'case').write_text(case)
        (root/'calls').write_text('')
        (root/'checks').write_text('')
        result=subprocess.run(['bwrap','--unshare-all','--uid','0','--ro-bind','/','/','--dev','/dev','--proc','/proc',
            '--tmpfs','/run','--tmpfs','/home','--tmpfs','/tmp','--dir','/tmp/fixture','--bind',str(root),'/tmp/fixture',
            *mocks,'/bin/bash','/tmp/fixture/scheduler',owner,CODE,'a'*64,'https://127.0.0.1:8443'],
            capture_output=True,timeout=10)
        assert (result.returncode==0)==success, (case,result.returncode)
        calls=[json.loads(line) for line in (root/'calls').read_text().splitlines()]
        schedules=[args for name,args in calls if name=='systemd-run']
        if success:
            assert len(schedules)==1 and '--unit=easymanage-platform-update-1001' in schedules[0]
            assert '--on-active=3s' in schedules[0] and 'install' in schedules[0]
            assert '--sha256' in schedules[0] and 'a'*64 in schedules[0]
            assert 'HOME=/home/tester' in schedules[0]
            checks=[json.loads(line) for line in (root/'checks').read_text().splitlines()]
            assert checks==[['check','--sha256','a'*64],['scheduled','--sha256','a'*64]]
        if case in ('wrong-unit','bad-candidate','root-owner'): assert not schedules
        assert not any(name=='systemctl' and any(action in args for action in ('stop','restart','kill')) for name,args in calls)
        assert not any('easymanage-agent.service' in args for _,args in calls)
        print('PASS isolated platform update scheduler',case,flush=True)
