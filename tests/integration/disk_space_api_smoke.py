"""Invoked by the existing isolated HTTPS/PostgreSQL fixture only."""
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import pwd
import subprocess
import time
import urllib.error
import urllib.request
import uuid


def exercise(root, base_url, context, enrollment, require):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    def request(path, body=None, headers=None):
        req=urllib.request.Request(base_url+path,data=None if body is None else json.dumps(body).encode(),
            headers={'Content-Type':'application/json','Origin':base_url}|(headers or {}))
        try: response=opener.open(req,timeout=15)
        except urllib.error.HTTPError as error:response=error
        with response:
            raw=response.read()
            try:value=json.loads(raw)
            except ValueError:value=raw.decode('utf-8',errors='replace')
            return response.status,value
    node=str(uuid.uuid4());username=pwd.getpwuid(os.getuid()).pw_name
    inventory={'hostname':'disk-space-fixture','os_name':'Linux','os_version':'fixture','kernel_version':'fixture',
               'architecture':'x86_64','cpu_model':'fixture','cpu_logical_cores':1,'cpu_physical_cores':1}
    status,identity=request('/api/v1/agent/enroll',{'protocol_version':1,'requested_node_id':node,'agent_version':'fixture','inventory':inventory},
                            {'x-easymanage-enrollment-token':enrollment})
    require(status==200,'disk scan fixture enrollment')
    template=next(n['metrics'] for n in request('/api/v1/nodes')[1]['nodes'] if n.get('metrics'))
    metrics=template|{'collected_at':datetime.now(timezone.utc).isoformat(),'privilege':{'mode':'basic','username':username,'uid':os.getuid()},'agent_control':None}
    report={'protocol_version':1,'node_id':node,'agent_version':'fixture','inventory':inventory,'metrics':metrics}
    require(request('/api/v1/agent/report',report,{'x-easymanage-agent-token':identity['agent_token']})[0]==200,'disk scan fixture heartbeat')
    registry=root/'targets.json';targets=json.loads(registry.read_text())
    targets['targets'].append({'node_id':node,'hostname':'disk-space-fixture','host':'198.51.100.2','port':22,
        'default_user':username,'agent_home':'/home/'+username+'/.easymanage','known_hosts':str(root/'known_hosts'),'ssh_target':'local'})
    registry.write_text(json.dumps(targets));registry.chmod(0o600)
    base=f'/api/v1/nodes/{node}/disk-space'
    require(request(base)[0]==200,'disk scan history API and additive migration available')
    require(request(base)[1]['limits']['seconds'] is None and request(base)[1]['limits']['entries'] is None,'unlimited scan capabilities')
    body={'path':'/tmp/work/tests/integration/fixtures','mode':'basic','seconds':0,'max_entries':0}
    require(request(base,body,{'Origin':'https://foreign.invalid'})[0]==403,'disk scan cross-origin refused')
    for change in [{'path':'/../etc'},{'path':'relative'},{'seconds':-1},{'max_entries':-1},{'mode':'sudo'},{'command':'rm'},{'scope':'network'},{'scope':'all_local'}]:
        require(request(base,body|change)[0] in (400,422),'invalid disk scan scope/budget/command rejected')
    require(request(base,body|{'mode':'full'})[0]==400,'Basic Agent cannot request root scan')
    low_space=Path('/tmp/easymanage-low-space')
    if low_space.is_dir():
        guard=root/'storage-guard'
        before_scans=request(base)[1]['scans']
        guard.unlink();guard.symlink_to(low_space,target_is_directory=True)
        try:
            code,blocked=request(base,body)
            require(code==507 and '2 GiB' in json.dumps(blocked,ensure_ascii=False),'low backing-filesystem space refuses scan before starting a worker')
            require(request(base)[1]['scans']==before_scans,'low-space refusal creates no failed scan or directory rows')
        finally:
            guard.unlink();guard.symlink_to(root,target_is_directory=True)
    status,created=request(base,body);require(status==200,'read-only Basic scan accepted')
    id=created['id'];deadline=time.monotonic()+20
    while time.monotonic()<deadline:
        code,record=request(base+'/'+id)
        if record['status']!='running':break
        time.sleep(.1)
    require(record['status']=='complete',f'actual fixed scanner completes through persisted task: {record.get("message", "")}')
    require(record['result']['uid']==os.getuid() and record['result']['root']==body['path']
            and record['result']['paged'] and len(record['result']['nodes'])==1,'scan summary contains real metadata and original user identity')
    code,page=request(base+'/'+id+'/entries')
    require(code==200 and len(page['entries'])<=250 and page['directory']==record['result']['nodes'][0],
            'directory summaries persist separately with matching totals')
    require(record['result']['detail_level']=='directories'
            and all(n['kind']=='directory' for n in page['entries'])
            and page['direct']['files']>=0,'only directories are returned; direct file totals remain available')
    require(request(base+'/'+id+'/entries?sort=allocated;DROP')[0]==400,'sort input is whitelisted')
    require(request(base+'/'+id+'/entries?parent=-1')[0]==400,'invalid parent rejected')
    require('hidden' not in json.dumps(record),'scan contains only requested directory metadata')
    history=request(base)[1]
    require(history['scans'][0]['id']==id and 'nodes' not in json.dumps(history),'history response is bounded summary, not repeated full trees')
    require(request(base+'/'+id+'/cancel',{})[0]==200,'completed scan cancellation is idempotent')
    # Stop before discovery/traversal: all-local routing and persistence can be
    # tested without enumerating any host filesystem exposed by the sandbox.
    code,all_job=request(base,body|{'path':'/','scope':'all_local','max_entries':1})
    require(code==200,'all-local job accepted with explicit scope')
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        all_scan=request(base+'/'+all_job['id'])[1]
        if all_scan['status']!='running':break
        time.sleep(.1)
    require(all_scan['scope']=='all_local' and all_scan['result']['scope']=='all_local'
            and all_scan['status']=='partial' and all_scan['result']['reason']=='entry_limit'
            and all_scan['result']['visited']==1,'all-local budget is global and scope survives streaming persistence')
    # A large synthetic snapshot verifies global ordering, pagination and ancestry
    # independently of filesystem size; this database exists only in the fixture.
    large=str(uuid.uuid4())
    sql=f"""INSERT INTO disk_space_scans(id,node_id,mode,root_path,status) VALUES('{large}','{node}','basic','/fixture','complete');
    INSERT INTO disk_space_directories VALUES('{large}',0,NULL,'/fixture','directory',1000000000,1000000000,26000,1,NULL,false,0,NULL);
    INSERT INTO disk_space_directories SELECT '{large}',i,0,'dir-'||lpad(i::text,5,'0'),'directory',i,i*2,1,0,NULL,false,0,NULL FROM generate_series(1,26000) i;
    INSERT INTO disk_space_directories VALUES('{large}',26001,0,'child','directory',1,1,0,0,NULL,false,0,NULL);"""
    subprocess.run(['/usr/lib/postgresql/18/bin/psql','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c',sql],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    page=request(base+'/'+large+'/entries')[1]
    require(page['direct']['allocated']==1000000000-sum(range(1,26001))-1 and page['direct']['files']==0,
            'direct contribution excludes all child directories, not just the current page')
    require(page['total']==26001 and len(page['entries'])==250 and page['entries'][0]['id']==26000,'large snapshot globally sorted with bounded page')
    second=request(base+'/'+large+'/entries?offset=250')[1]
    require(second['entries'][0]['id']==25750,'second page continues full-directory ordering')
    last=request(base+'/'+large+'/entries?offset=26000')[1]
    require(len(last['entries'])==1 and last['entries'][0]['name']=='child','all entries accessible past old 20,000 limit')
    filtered=request(base+'/'+large+'/entries?search=dir-00001')[1]
    require(filtered['total']==1 and filtered['entries'][0]['id']==1 and filtered['direct']==page['direct'],'search covers entries beyond first page')
    child=request(base+'/'+large+'/entries?parent=26001')[1]
    require([e['id'] for e in child['ancestors']]==[0,26001] and child['total']==0,'directory ancestry and empty-directory response')
    require(request(base,body|{'path':'/proc'})[0]==200,'unsupported filesystem failure is tracked as a job')
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        rows=request(base)[1]['scans'];failed=next(s for s in rows if s['root_path']=='/proc')
        if failed['status']!='running':break
        time.sleep(.1)
    require(failed['status']=='failed' and '目录不可读取' in failed['message'],'virtual filesystem rejected without traversal')
    # Simulate PostgreSQL's actual disk-full SQLSTATE without filling any disk.
    failure_sql="""CREATE FUNCTION fixture_disk_full() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'synthetic private storage path' USING ERRCODE='53100'; END $$;
    CREATE TRIGGER fixture_disk_full BEFORE INSERT ON disk_space_directories FOR EACH ROW EXECUTE FUNCTION fixture_disk_full();"""
    psql=['/usr/lib/postgresql/18/bin/psql','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c']
    subprocess.run(psql+[failure_sql],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    try:
        code,job=request(base,body);require(code==200,'disk-full fixture scan accepted before simulated write failure')
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            failed=request(base+'/'+job['id'])[1]
            if failed['status']!='running':break
            time.sleep(.1)
        require(failed['status']=='failed' and '磁盘空间已耗尽' in failed['message']
                and 'synthetic private storage path' not in json.dumps(failed),'disk-full error names the cause without exposing database diagnostics')
    finally:
        subprocess.run(psql+['DROP TRIGGER fixture_disk_full ON disk_space_directories; DROP FUNCTION fixture_disk_full();'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    metrics['collected_at']=datetime.now(timezone.utc).isoformat();metrics['privilege']={'mode':'full','username':'root','uid':0}
    require(request('/api/v1/agent/report',report,{'x-easymanage-agent-token':identity['agent_token']})[0]==200,'Full fixture authorization boundary heartbeat')
    require(request(base,body|{'mode':'full'})[0] in (401,403),'Full Agent root scan still requires browser administrator authorization')
    require(request(base)[1]['root_authorized'] is False,'ordinary browser has no root execution authorization')
    shared_id=str(uuid.uuid4())
    shared_root={'id':0,'parent':None,'name':'/root/SHARED_SCAN_FIXTURE','kind':'directory','allocated':1000,'apparent':2000,'files':3,'directories':1,'issue':None,'incomplete':False,'mtime':0,'scan_path':None}
    shared_child=shared_root|{'id':1,'parent':0,'name':'nested','allocated':600,'apparent':900,'files':2,'directories':0}
    shared_result={'root':shared_root['name'],'uid':0,'username':'root','status':'complete','reason':'','elapsed_seconds':1,
                   'filesystem':'ext4','visited':5,'errors':0,'skipped_mounts':0,'symlinks':0,'hardlinks':0,
                   'nodes':[shared_root],'paged':True,'detail_level':'directories','saved_directories':2}
    encoded=json.dumps(shared_result).replace("'","''")
    directories=json.dumps([shared_root,shared_child]).replace("'","''")
    seed=f"INSERT INTO disk_space_scans(id,node_id,mode,root_path,status,result) VALUES('{shared_id}','{node}','full','/root/SHARED_SCAN_FIXTURE','complete','{encoded}'::jsonb);"
    seed+=f"INSERT INTO disk_space_directories SELECT '{shared_id}',n.* FROM jsonb_to_recordset('{directories}'::jsonb) AS n(id BIGINT,parent BIGINT,name TEXT,kind TEXT,allocated BIGINT,apparent BIGINT,files BIGINT,directories BIGINT,issue TEXT,incomplete BOOLEAN,mtime BIGINT,scan_path TEXT);"
    subprocess.run(psql+[seed],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    code,history=request(base)
    require(code==200 and any(s['id']==shared_id and s['mode']=='full' for s in history['scans'])
            and history['root_authorized'] is False,'ordinary browser lists root snapshots without receiving root execution authorization')
    code,shared=request(base+'/'+shared_id)
    require(code==200 and shared['result']==shared_result,'ordinary browser reads existing root scan totals without credentials')
    code,shared_page=request(base+'/'+shared_id+'/entries')
    require(code==200 and shared_page['directory']==shared_root and shared_page['entries']==[shared_child]
            and shared_page['direct']=={'allocated':400,'apparent':1100,'files':1},'ordinary browser reads root directory hierarchy and direct file totals')
    code,nested=request(base+'/'+shared_id+'/entries?parent=1')
    require(code==200 and [d['id'] for d in nested['ancestors']]==[0,1], 'ordinary browser drills into root snapshot directories')
    require(request(f'/api/v1/nodes/{uuid.uuid4()}/disk-space/{shared_id}')[0]==404,'shared results remain scoped to the original machine')
    require(request(base+'/'+shared_id+'/cancel',{})[0]==401,'reading root results does not authorize cancellation')
    require(request(base+'/'+shared_id+'/cancel',{}, {'Origin':'https://foreign.invalid'})[0]==403,'root cancellation retains origin validation')
    subprocess.run(psql+[f"UPDATE disk_space_scans SET status='running' WHERE id='{shared_id}'"],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    try:
        require(request(base+'/'+shared_id)[1]['status']=='running','ordinary browser can monitor root scan progress')
        require(request(base+'/'+shared_id+'/cancel',{})[0]==401
                and request(base+'/'+shared_id)[1]['status']=='running','ordinary read access cannot stop an active root scan')
    finally:
        subprocess.run(psql+[f"UPDATE disk_space_scans SET status='complete' WHERE id='{shared_id}'"],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    exercise_summary_migration(node,require)
    print('PASS isolated disk-space HTTPS, PostgreSQL persistence, actual fixed scanner, permissions, input/origin checks, filesystem refusal and small history responses',flush=True)


def exercise_summary_migration(node,require):
    # Recreate old scan storage in a private schema of the already-isolated
    # database. A transaction rollback removes this fixture, not production data.
    old=Path('migrations/0013_disk_space_scans.sql').read_text()+Path('migrations/0014_disk_space_streaming.sql').read_text()+Path('migrations/0015_disk_space_scope.sql').read_text()
    migration=Path('migrations/0016_disk_space_directory_summaries.sql').read_text()
    first,legacy,failed=[str(uuid.uuid4()) for _ in range(3)]
    root={'id':0,'parent':None,'name':'/fixture','kind':'directory','allocated':1000,'apparent':2000,'files':2,'directories':1,'issue':None,'incomplete':False,'mtime':0,'scan_path':None}
    child=root|{'id':2,'parent':0,'name':'child','allocated':300,'apparent':600,'files':1,'directories':0}
    private=root|{'id':1,'parent':0,'name':'PRIVATE_FILE_MUST_DISAPPEAR','kind':'file','allocated':700,'apparent':1400,'files':1,'directories':0}
    snapshots=[(first,[root]),(legacy,[root,private,child])]
    seed=''
    for id,nodes in snapshots:
        value=json.dumps({'nodes':nodes,'visited':4,'status':'complete','paged':id==first}).replace("'","''")
        seed+=f"INSERT INTO disk_space_scans(id,node_id,mode,root_path,status,result) VALUES('{id}','{node}','basic','/fixture','complete','{value}'::jsonb);"
    seed+=f"INSERT INTO disk_space_scans(id,node_id,mode,root_path,status) VALUES('{failed}','{node}','basic','/fixture','failed');"
    for id,items in [(first,[root,private,child]),(failed,[root,private])]:
        value=json.dumps(items).replace("'","''")
        seed+=f"INSERT INTO disk_space_entries SELECT '{id}',n.* FROM jsonb_to_recordset('{value}') AS n(id BIGINT,parent BIGINT,name TEXT,kind TEXT,allocated BIGINT,apparent BIGINT,files BIGINT,directories BIGINT,issue TEXT,incomplete BOOLEAN,mtime BIGINT,scan_path TEXT);"
    checks=f"""
    DO $$ BEGIN
      IF to_regclass('summary_migration_fixture.disk_space_entries') IS NOT NULL THEN RAISE EXCEPTION 'old table survives'; END IF;
      IF (SELECT COUNT(*) FROM disk_space_directories)<>4 THEN RAISE EXCEPTION 'directory count differs'; END IF;
      IF EXISTS(SELECT 1 FROM disk_space_directories WHERE kind<>'directory') THEN RAISE EXCEPTION 'file row survives'; END IF;
      IF EXISTS(SELECT 1 FROM disk_space_scans WHERE result::text LIKE '%PRIVATE_FILE_MUST_DISAPPEAR%') THEN RAISE EXCEPTION 'JSON file survives'; END IF;
      IF (SELECT COUNT(*) FROM disk_space_scans WHERE result->>'detail_level'='directories' AND result->>'saved_directories'='2' AND result->>'paged'='true' AND (result->'nodes'->0->>'allocated')::BIGINT=1000)<>2 THEN RAISE EXCEPTION 'root totals changed'; END IF;
      IF (SELECT COUNT(*) FROM disk_space_directories WHERE id=2 AND parent=0 AND allocated=300 AND files=1)<>2 THEN RAISE EXCEPTION 'sparse directory identities changed'; END IF;
      IF (SELECT COUNT(*) FROM pg_indexes WHERE schemaname='summary_migration_fixture' AND tablename='disk_space_directories')<>2 THEN RAISE EXCEPTION 'redundant indexes retained'; END IF;
      IF (SELECT COUNT(*) FROM disk_space_scans WHERE id='{failed}' AND status='failed')<>1 THEN RAISE EXCEPTION 'failed task history changed'; END IF;
      BEGIN
        INSERT INTO disk_space_directories VALUES('{first}',3,0,'invalid','file',1,1,1,0,NULL,false,0,NULL);
        RAISE EXCEPTION 'database accepted file row';
      EXCEPTION WHEN check_violation THEN NULL; END;
    END $$;
    """
    sql="BEGIN; CREATE SCHEMA summary_migration_fixture; SET LOCAL search_path=summary_migration_fixture,public;"+old+seed+migration+checks+"ROLLBACK;"
    result=subprocess.run(['/usr/lib/postgresql/18/bin/psql','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c',sql],capture_output=True,text=True)
    require(result.returncode==0,'directory-only migration preserves old totals/history, converts JSON, drops file rows and indexes: '+result.stderr[-1200:])
    print('PASS legacy and streaming snapshot conversion, file metadata removal, directory totals/IDs, two indexes and schema refusal of file rows',flush=True)
