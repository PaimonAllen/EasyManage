#!/usr/bin/env python3
"""Fixed service shutdown in private nested namespaces, never host systemd."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import uuid

for namespace, original in zip(('net','mnt'),sys.argv[1:]):
    assert os.readlink('/proc/self/ns/'+namespace)!=original
assert len(sys.argv)==3

MOCK='''#!/usr/bin/python3
import json,sys
from pathlib import Path
r=Path('/tmp/fixture');args=sys.argv[1:];user='--user' in args
if user:args.remove('--user')
case=(r/'case').read_text()
with (r/'calls').open('a') as f:f.write(json.dumps(args)+'\\n')
assert 'easymanage-server.service' not in args
full=case.startswith('full')
if not full and not user:
 if args[0]=='show':print('inactive');sys.exit(0)
 sys.exit(1)
if args[0]=='show':
 field=args[args.index('-p')+1]
 values={'LoadState':'loaded' if full or case=='basic-systemd' else 'not-found','User':'other' if case=='full-wrong-user' else 'root',
 'ExecStart':'{ path='+('/usr/local/sbin/easymanage-agent' if full else '/home/tester/.easymanage/bin/easymanage-agent')+' ; }',
 'ActiveState':'inactive' if (r/'disabled').exists() else 'active','MainPID':'0' if (r/'disabled').exists() else '123'}
 print(values[field]);sys.exit(0)
if args[0]=='disable':(r/'disabled').touch();sys.exit(0)
if args[0]=='is-enabled':print('disabled' if (r/'disabled').exists() else 'enabled');sys.exit(1 if (r/'disabled').exists() else 0)
sys.exit(1)
'''

with tempfile.TemporaryDirectory(dir='/tmp',prefix='agent-stop.') as temporary:
    root=Path(temporary);script=Path('deploy/linux/operations/stop-agent-service.py').resolve()
    control=Path('deploy/linux/user/agent-user-control.py').resolve()
    for case in ['full','full-wrong-id','full-wrong-user','basic-systemd','basic-compatibility']:
        fixture=root/case;fixture.mkdir();(fixture/'case').write_text(case);(fixture/'calls').write_text('')
        mock=fixture/'systemctl';mock.write_text(MOCK);mock.chmod(0o755)
        base=fixture/'agent';(base/'config').mkdir(parents=True);(base/'state').mkdir();(base/'bin').mkdir();(base/'logs').mkdir()
        node=str(uuid.uuid4());is_full=case.startswith('full')
        real_base='/var/lib/easymanage-agent' if is_full else '/home/tester/.easymanage'
        db=base/'state/state.db'
        with sqlite3.connect(db) as connection:
            connection.execute('CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT)')
            connection.executemany('INSERT INTO metadata VALUES(?,?)',[('node_id',node),('retained-history','fixture-sentinel')])
        env=base/'config/agent.env';env.write_text('EASYMANAGE_NODE_ID='+node+'\nEASYMANAGE_AGENT_STATE_DB='+real_base+'/state.db\n' if is_full else 'EASYMANAGE_NODE_ID='+node+'\nEASYMANAGE_AGENT_STATE_DB='+real_base+'/state/state.db\n')
        (base/'bin/agent-user-control.py').write_bytes(control.read_bytes())
        binary=base/'bin/easymanage-agent';binary.write_text('#!/usr/bin/python3\nimport time\nwhile True:time.sleep(1)\n');binary.chmod(0o700)
        (base/'config/agent-launch.json').write_text(json.dumps({'backend':'compatibility' if case=='basic-compatibility' else 'systemd','autostart':'manual'}))
        before=db.read_bytes();before_env=env.read_bytes()
        arguments=['bwrap','--unshare-all','--die-with-parent','--new-session','--ro-bind','/','/','--dev','/dev','--proc','/proc','--tmpfs','/run','--tmpfs','/home','--tmpfs','/var','--tmpfs','/etc/easymanage','--tmpfs','/etc/systemd/system','--tmpfs','/tmp','--dir','/tmp/fixture','--bind',str(fixture),'/tmp/fixture','--ro-bind',str(script),'/tmp/stop.py','--ro-bind',str(mock),'/usr/bin/systemctl']
        if is_full:
            arguments+=['--uid','0','--dir','/var/lib/easymanage-agent','--bind',str(base/'state'),'/var/lib/easymanage-agent','--ro-bind',str(env),'/etc/easymanage/agent.env']
            invocation=['/usr/bin/python3','/tmp/stop.py','full',str(uuid.uuid4()) if case=='full-wrong-id' else node,'/home/tester/.easymanage','tester']
        else:
            passwd=fixture/'passwd';passwd.write_text(f'tester:x:{os.getuid()}:{os.getgid()}:fixture:/home/tester:/bin/bash\n')
            arguments+=['--ro-bind',str(passwd),'/etc/passwd','--dir','/home/tester','--bind',str(base),'/home/tester/.easymanage','--setenv','HOME','/home/tester']
            driver="""import subprocess,sys,time,pathlib
b=pathlib.Path('/home/tester/.easymanage');node=sys.argv[1];compat=sys.argv[2]=='yes'
if compat:
 subprocess.run(['/usr/bin/python3',str(b/'bin/agent-user-control.py'),'start'],check=True)
 time.sleep(.3)
subprocess.run(['/usr/bin/python3','/tmp/stop.py','basic',node,str(b),'tester'],check=True)
if compat:
 assert subprocess.run(['/usr/bin/python3',str(b/'bin/agent-user-control.py'),'status']).returncode==3
 subprocess.run(['/usr/bin/python3',str(b/'bin/agent-user-control.py'),'start','--enabled-only'],check=True)
 assert subprocess.run(['/usr/bin/python3',str(b/'bin/agent-user-control.py'),'status']).returncode==3
 assert not (b/'state/compat.enabled').exists()
"""
            invocation=['/usr/bin/python3','-c',driver,node,'yes' if case=='basic-compatibility' else 'no']
        result=subprocess.run(arguments+invocation,capture_output=True,timeout=45)
        success=case not in ['full-wrong-id','full-wrong-user']
        assert (result.returncode==0)==success,(case,result.returncode,result.stderr.decode())
        assert db.read_bytes()==before and env.read_bytes()==before_env,'Identity/history/config changed'
        calls=[json.loads(line) for line in (fixture/'calls').read_text().splitlines()]
        disabled=any(args[0]=='disable' for args in calls)
        assert disabled==(success and case!='basic-compatibility'),case
        assert all('easymanage-server.service' not in args for args in calls)
        print('PASS fixed Agent stop, preserved files/history and no auto-respawn:',case,flush=True)
