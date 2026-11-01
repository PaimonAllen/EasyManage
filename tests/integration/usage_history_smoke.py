"""Synthetic seven-day history and resource isolation in the private fixture."""
from datetime import datetime,timedelta,timezone
import json
import subprocess
import threading
import time
import urllib.error
import urllib.request
import uuid


def exercise(root,base_url,context,enrollment,require):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    def request(path,body=None,headers=None):
        req=urllib.request.Request(base_url+path,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'}|(headers or {}))
        try:response=opener.open(req,timeout=15)
        except urllib.error.HTTPError as e:response=e
        with response:
            raw=response.read()
            try:value=json.loads(raw)
            except ValueError:value=raw.decode(errors='replace')
            return response.status,value,{k.lower():v for k,v in response.headers.items()}
    psql=['/usr/lib/postgresql/18/bin/psql','-X','-A','-t','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c']
    def database(sql):
        return subprocess.check_output(psql+[sql],text=True,stderr=subprocess.DEVNULL).strip()
    node=str(uuid.uuid4())
    inventory={'hostname':'usage-history-fixture','os_name':'Linux','os_version':'fixture','kernel_version':'fixture','architecture':'x86_64','cpu_model':'fixture','cpu_logical_cores':1,'cpu_physical_cores':1}
    status,identity,_=request('/api/v1/agent/enroll',{'protocol_version':1,'requested_node_id':node,'agent_version':'fixture','inventory':inventory},{'x-easymanage-enrollment-token':enrollment})
    require(status==200,'usage-history fixture enrollment')
    now=datetime.now(timezone.utc)
    local=(now+timedelta(hours=8)).date()
    start=datetime.combine(local-timedelta(days=6),datetime.min.time(),tzinfo=timezone.utc)-timedelta(hours=8)
    t=start+timedelta(hours=1)
    def gpu(index,core,memory):return {'index':index,'name':'Fixture GPU '+str(index),'utilization_percent':core,'memory_total_bytes':100,'memory_used_bytes':memory,'temperature_celsius':75,'power_watts':None}
    def insert(at,cpu,gpus,gpu_at=None,disk_at=None,read=None,written=None,thermal=None):
        gpu_json=json.dumps(gpus).replace("'","''")
        thermal='NULL' if thermal is None else "'"+json.dumps(thermal).replace("'","''")+"'::jsonb"
        timestamp=lambda value:'NULL' if value is None else "'"+value.isoformat()+"'::timestamptz"
        return f"INSERT INTO metrics_samples(node_id,collected_at,fast_collected_at,cpu_usage_percent,memory_used_bytes,network_received_bytes_per_second,network_transmitted_bytes_per_second,gpu_metrics,gpu_collected_at,disk_collected_at,disk_read_bytes_per_second,disk_written_bytes_per_second,temperatures) VALUES('{node}',{timestamp(at)},{timestamp(at)},{cpu},{int(cpu/10)*1024**3},{cpu*10},{cpu*20},'{gpu_json}'::jsonb,{timestamp(gpu_at)},{timestamp(disk_at)},{'NULL' if read is None else read},{'NULL' if written is None else written},{thermal});"
    thermal=lambda at,cpu,ssd:{'collected_at':at.isoformat(),'cpu_celsius':cpu,'ssds':[{'device':'fixture','temperature_celsius':ssd}]}
    old=[gpu(0,10,40),gpu(7,80,50)]
    seed=insert(t,10,old,t,t,20,None,thermal(t,55,42))
    seed+=insert(t+timedelta(seconds=10),50,old,t,t,20,None,thermal(t,55,42))
    last=t+timedelta(seconds=20)
    seed+=insert(last,90,[gpu(7,60,70)|{'thermal_throttling':True},gpu(0,30,60)|{'thermal_throttling':False}],last,last,40,80,thermal(last,61,45)|{'cpu_thermal_throttling':True})
    tomorrow=t+timedelta(days=1)
    seed+=insert(tomorrow,0,[gpu(0,0,0)|{'thermal_throttling':False}],tomorrow)
    seed+=insert(start-timedelta(seconds=1),100,[gpu(99,100,100)],start-timedelta(seconds=1))
    seed+=insert(now+timedelta(days=1),100,[gpu(98,100,100)],now+timedelta(days=1))
    database(seed)
    base=f'/api/v1/nodes/{node}/usage-history'
    require(request('/usage-history?node='+node)[0]==200 and request('/usage-history/?node='+node)[0]==200,'standalone history page has explicit successful routes for direct links')
    for suffix in ['?utc_offset_minutes=841','?utc_offset_minutes=1','?days=365']:
        require(request(base+suffix)[0]==400,'history refuses unbounded or invalid request parameters')
    code,data,headers=request(base+'?utc_offset_minutes=480')
    require(code==200 and headers.get('x-history-cache')=='miss','first seven-day read builds an on-demand aggregate')
    require(data['source_rows']==4 and len(data['days'])==7 and len(data['points'])==2,'calendar window excludes expired/future rows and bounds the plotted buckets')
    require(data['start'].startswith(start.isoformat()[:19]) and data['days'][0]['date']==str(local-timedelta(days=6)),'history day boundaries use the requested fixed UTC offset')
    require(data['summary']['cpu']['average']==37.5 and data['summary']['cpu']['maximum']==90 and data['summary']['cpu']['samples']==4,'CPU aggregate retains averages, peaks and real idle samples')
    require(data['summary']['disk_read']['average']==30 and data['summary']['disk_read']['samples']==2 and data['summary']['disk_write']['average']==80,'cached disk timestamps deduplicate and missing writes are not zero')
    require(data['summary']['cpu_temperature']['samples']==2 and data['summary']['cpu_temperature']['maximum']==61,'temperature history deduplicates actual sensor collection times')
    require([g['index'] for g in data['gpus']]==[0,7] and data['summary']['gpus']['7']['core']['average']==70 and data['summary']['gpus']['7']['core']['samples']==2,'historical GPU indices survive disappearance and array reordering')
    require('7' not in data['days'][1]['summary']['gpus'] and data['days'][2]['summary']['cpu']['average'] is None,'missing GPU/day data remains absent instead of invented zeros')
    require(data['summary']['gpu_count']['minimum']==1 and data['summary']['gpu_count']['maximum']==2,'valid GPU count changes remain visible')
    require(data['summary']['cpu_thermal_throttling']=={'samples':1,'detected_samples':1} and data['summary']['gpu']['thermal_throttling']=={'samples':2,'detected_samples':1},'thermal throttling requires hardware evidence; old temperature-only history remains unknown')
    require(data['summary']['gpus']['7']['thermal_throttling']=={'samples':1,'detected_samples':1} and data['summary']['gpus']['0']['thermal_throttling']['detected_samples']==0,'GPU thermal throttle flags remain independent, ignore temperature and deduplicate cached reports')
    database(insert(t+timedelta(seconds=30),100,[],None))
    code,cached,headers=request(base+'?utc_offset_minutes=480')
    require(code==200 and headers.get('x-history-cache')=='hit' and cached==data,'repeated viewers reuse the serialized cache without scanning new rows')
    require(int(database("SELECT COUNT(*) FROM pg_stat_activity WHERE application_name='easymanage-usage-history'"))==1,'history uses at most one separate lazy database connection')
    require(request(f'/api/v1/nodes/{uuid.uuid4()}/usage-history')[0]==404,'unknown machine history is not fabricated')
    # Lock only the disposable fixture's history table. A history reader waits
    # briefly; ordinary monitoring's connection pool remains available.
    lock=subprocess.Popen(psql+["BEGIN; LOCK TABLE metrics_samples IN ACCESS EXCLUSIVE MODE; SELECT pg_sleep(2); ROLLBACK;"],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    try:
        deadline=time.monotonic()+1
        while time.monotonic()<deadline:
            if database("SELECT COUNT(*) FROM pg_locks WHERE relation='metrics_samples'::regclass AND mode='AccessExclusiveLock' AND granted")!='0':break
            time.sleep(.005)
        result=[]
        thread=threading.Thread(target=lambda:result.append(request(base+'?utc_offset_minutes=0')))
        thread.start()
        deadline=time.monotonic()+1
        while time.monotonic()<deadline:
            if database("SELECT COUNT(*) FROM pg_stat_activity WHERE application_name='easymanage-usage-history' AND wait_event_type='Lock'")!='0':break
            time.sleep(.005)
        require(request(base+'?utc_offset_minutes=60')[0]==429,'concurrent uncached history queries are rejected instead of queued')
        before=time.monotonic();code,_,_=request('/api/v1/health/ready')
        require(code==200 and time.monotonic()-before<1,'monitoring readiness stays responsive while the history connection is blocked')
        thread.join(timeout=5)
        require(result and result[0][0]==503,'history lock timeout fails safely without holding the monitoring pool')
    finally:lock.wait(timeout=5)
    print('PASS standalone seven-day usage history, calendar bounds, GPU continuity, null gaps, cache and isolated query budgets',flush=True)
