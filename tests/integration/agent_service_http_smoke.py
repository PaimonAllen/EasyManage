"""Actual private Basic Agent and SSH transport shutdown/redeployment regression."""
import json
from pathlib import Path
import sqlite3
import subprocess
import time
import urllib.error
import urllib.request
import uuid


def exercise(request,wait_job,begin_redeployment,node,username,home,ssh_env,url,context,restart_controller,require,transports,max_connections):
    base=home/'.easymanage';endpoint=f'/api/v1/machines/{node}/agent/stop'
    def machine():return next(m for m in request('/api/v1/machines')[1]['machines'] if m['node_id']==node)
    def snapshot():return next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==node)
    def wait(predicate,seconds=30):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            value=predicate()
            if value:return value
            time.sleep(.2)
        raise AssertionError('Private lifecycle evidence did not arrive')
    def identity():
        with sqlite3.connect((base/'state/state.db').as_uri()+'?mode=ro',uri=True) as database:
            return dict(database.execute("SELECT key,value FROM metadata WHERE key IN ('node_id','agent_token','stop-retained-history')"))
    with sqlite3.connect(base/'state/state.db',timeout=5) as database:
        database.execute("INSERT INTO metadata(key,value) VALUES('stop-retained-history','preserved-test-history')")
    original=identity();original_pid=json.loads((base/'state/compat-process.json').read_text())['agent_pid']
    current=machine();revision=current['monitoring_revision']
    code,result=request(f'/api/v1/machines/{node}/monitoring',{'confirmation':node,'paused':True,'expected_revision':revision})
    require(code==200,'pause stores metrics intent independently of process shutdown')
    wait(lambda:machine()['monitoring_applied_revision']==revision+1)
    require(machine()['control_seen_at'] is not None and json.loads((base/'state/compat-process.json').read_text())['agent_pid']==original_pid,
            'paused monitoring retains the real Agent process and authenticated lightweight heartbeat')
    paused_seen=snapshot()['last_seen'];paused_control=machine()['control_seen_at']
    wait(lambda:machine()['control_seen_at']!=paused_control,75)
    require(snapshot()['last_seen']==paused_seen and not snapshot()['online']
            and json.loads((base/'state/compat-process.json').read_text())['agent_pid']==original_pid,
            'a full paused minute advances only the control heartbeat, without metrics reports or process restart')
    require(request(f'/api/v1/machines/{node}/monitoring',{'confirmation':node,'paused':False,'expected_revision':revision+1})[0]==200,
            'paused Agent resumes directly without installation or SSH reauthentication')
    wait(lambda:snapshot()['online'],75)
    require(identity()==original and json.loads((base/'state/compat-process.json').read_text())['agent_pid']==original_pid,
            'pause/resume retains exact identity and process PID')
    history={p['collected_at'] for p in request(f'/api/v1/nodes/{node}/history')[1]['points']}
    body={'confirmation':node,'expected_revision':0,'credentials':{'use_saved':True}}
    require(request(endpoint,body|{'confirmation':str(uuid.uuid4())})[0]==400,'complete shutdown rejects another confirmation before SSH')
    no_cookie=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    try:
        no_cookie.open(urllib.request.Request(url+endpoint,data=json.dumps(body).encode(),headers={'Origin':url,'Content-Type':'application/json'}),timeout=5)
    except urllib.error.HTTPError as error:
        require(error.code==401,'Agent stop cannot reuse encrypted credentials without the scoped browser capability')
    else:raise AssertionError('Unauthorized stop accepted')
    before=len(transports)
    code,result=request(endpoint,body)
    if code!=200:print('DIAGNOSTIC private stop:',code,result,flush=True)
    require(code==200 and '重新部署' in result.get('message','') and '心跳' in result.get('message',''),'real SSH Agent stop completes with verified service shutdown')
    require(machine()['agent_service_state']=='stopped' and machine()['agent_service_desired_stopped'] and not snapshot()['online'],
            'verified stop is distinct from paused monitoring and immediately excludes live availability')
    require(identity()==original and (base/'bin/easymanage-agent').is_file() and (base/'config/agent.env').is_file(),
            'complete stop retains the original program, configuration, SQLite identity and local history')
    require(subprocess.run(['/usr/bin/python3',str(base/'bin/agent-user-control.py'),'status'],env=ssh_env,capture_output=True).returncode==3,
            'complete stop terminates the actual compatibility supervisor and Agent process')
    subprocess.run(['/usr/bin/python3',str(base/'bin/agent-user-control.py'),'start','--enabled-only'],env=ssh_env,check=True,capture_output=True)
    require(subprocess.run(['/usr/bin/python3',str(base/'bin/agent-user-control.py'),'status'],env=ssh_env,capture_output=True).returncode==3,
            'cron enabled-only recovery cannot respawn a stopped Agent')
    wait(lambda:machine()['ssh_transport']['connections']==0,8)
    require(len(transports)==before,'shutdown reuses the original SSH session rather than dialing once per instruction')
    require(request(f'/api/v1/machines/{node}/monitoring',{'confirmation':node,'paused':False,'expected_revision':revision+2})[0]==409,
            'resume-monitoring cannot start a fully stopped Agent')
    for path,body2 in [(f'/api/v1/nodes/{node}/agent/restart',{}),(f'/api/v1/nodes/{node}/agent/force-restart',{}),
                       (f'/api/v1/machines/{node}/ssh',{'action':'connect','username':username,'use_saved':True})]:
        require(request(path,body2)[0]==409,'restart or SSH reconnect cannot bypass required redeployment after a stop')
    code,created=begin_redeployment(node,{'confirmation':node,'username':username,'use_saved':True,'connection_mode':'ssh','server_url':url})
    require(code==200,'stopped node is eligible for immediate original-identity redeployment without waiting for heartbeat expiry')
    job=wait_job(created)
    if job['status']!='success':print('DIAGNOSTIC private stopped-node redeploy:',[e['message'] for e in job['events']],flush=True)
    require(job['status']=='success','stopped Agent resumes only through real SCP/checksum installation and a new verified heartbeat')
    wait(lambda:snapshot()['online'])
    require(machine()['agent_service_state']=='running' and not machine()['agent_service_desired_stopped']
            and identity()['node_id']==original['node_id'] and identity()['stop-retained-history']==original['stop-retained-history'],
            'redeployment retains UUID/local history and clears the stopped state only after real new-process evidence')
    require(history<={p['collected_at'] for p in request(f'/api/v1/nodes/{node}/history')[1]['points']},'stopped-node redeployment preserves all platform history')
    stopped=machine()
    code,_=request(endpoint,{'confirmation':node,'expected_revision':stopped['agent_service_revision'],'credentials':{'use_saved':True}})
    require(code==200,'redeployed Agent can be stopped again without repeated authentication')
    wait(lambda:machine()['ssh_transport']['connections']==0,8)
    before=len(transports)
    restart_controller();time.sleep(2)
    require(machine()['agent_service_state']=='stopped' and machine()['ssh_transport']['connections']==0 and len(transports)==before,
            'platform restart retains complete stop and initiates no automatic SSH reconnect')
    require(max_connections[0]<=3,'all stop/redeploy operations remain within three physical SSH sessions')
