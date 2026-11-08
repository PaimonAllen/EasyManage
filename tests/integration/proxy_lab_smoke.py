"""Proxy analysis fixtures, called only by the private namespace smoke runner.

The fake core serves a local Unix socket and never opens an outbound connection.
Real core startup/cleanup is tested separately with the opt-in Rust test.
"""
import json,subprocess,time,urllib.request,urllib.error
from datetime import datetime,timezone

def prepare(root):
    path=root/'fixture-core'
    path.write_text('''#!/usr/bin/python3
import argparse,http.server,json,socketserver,os,signal,time,pathlib,threading,urllib.parse,base64
p=argparse.ArgumentParser();p.add_argument('-d');p.add_argument('-f');args=p.parse_args()
c=json.load(open(args.f))
root=pathlib.Path(__file__).parent
history=root/'core-events.jsonl'
probes=root/'site-events.jsonl'
lock=threading.Lock();extra={};active=0;per_node={}
def record(event):
 with history.open('a') as f:f.write(json.dumps({'event':event,'pid':os.getpid(),'nodes':[n['name'] for n in c['proxies']]})+'\\n')
def terminate(*_):
 signal.signal(signal.SIGTERM,signal.SIG_IGN)
 raise SystemExit()
signal.signal(signal.SIGTERM,terminate)
record('start')
if any(n.get('port')==457 for n in c['proxies']):record('stop');raise SystemExit(2)
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_GET(self):
  global active
  if self.headers.get('Authorization')!='Bearer '+c['secret']:self.send_error(401);return
  parsed=urllib.parse.urlparse(self.path);parts=parsed.path.strip('/').split('/')
  if self.path=='/version':value={'version':'private-fixture'}
  elif len(parts)==3 and parts[2]=='delay':
   node=parts[1];query=urllib.parse.parse_qs(parsed.query);url=query.get('url',[''])[0];port=next(n['port'] for n in c['proxies'] if n['name']==node)
   host=urllib.parse.urlparse(url).hostname
   with lock:
    active+=1;per_node[node]=per_node.get(node,0)+1
    with probes.open('a') as f:f.write(json.dumps({'node':node,'url':url,'timeout':query.get('timeout'),'expected':query.get('expected'),'active':active,'per_node':per_node[node]})+'\\n')
   time.sleep(2 if port==458 else .5)
   delay=1601 if port==465 else 17
   failed=(port==461 and host=='www.amazon.com') or (port in (462,467) and host in ('en.wikipedia.org','www.amazon.com'))
   wrong_status=port==463 and host in ('en.wikipedia.org','www.amazon.com')
   with lock:
    extra.setdefault(node,{})[url]={'alive':not(failed or wrong_status),'history':[{'delay':0 if failed or wrong_status else delay+(1 if port==468 and host=='www.amazon.com' else 0)}]}
    active-=1;per_node[node]-=1
   if failed:self.send_error(504);return
   if (port==466 and host in ('en.wikipedia.org','www.amazon.com')) or (port==467 and host=='www.google.com'):
    self.send_response(200);self.end_headers();self.wfile.write(b'not-json');return
   value={'delay':delay}
  elif len(parts)==2 and parts[0]=='proxies':
   node=parts[1];port=next(n['port'] for n in c['proxies'] if n['name']==node)
   with lock:value={'extra':dict(extra.get(node,{}))} if port!=464 else {}
  else:self.send_error(404);return
  body=json.dumps(value).encode()
  self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def do_PUT(self):
  if self.headers.get('Authorization')!='Bearer '+c['secret']:self.send_error(401);return
  if any(n.get('port')==459 for n in c['proxies']):time.sleep(2)
  self.send_response(204);self.end_headers()
class ProxyHandler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_CONNECT(self):
  raw=self.headers.get('Proxy-Authorization','')
  valid=raw.startswith('Basic ') and base64.b64decode(raw[6:]).decode() in c.get('authentication',[])
  self.send_response(502 if valid else 407);self.send_header('Content-Length','0');self.end_headers()
class ProxyServer(http.server.ThreadingHTTPServer):daemon_threads=True;request_queue_size=64
if c.get('mixed-port'):
 proxy=ProxyServer(('127.0.0.1',c['mixed-port']),ProxyHandler)
 threading.Thread(target=proxy.serve_forever,daemon=True).start()
class Server(socketserver.ThreadingMixIn,socketserver.UnixStreamServer):daemon_threads=True;request_queue_size=64
try:Server(c['external-controller-unix'],Handler).serve_forever()
finally:record('stop')
''')
    path.chmod(0o700)
    return path

def exercise(root,url,context,require):
    base='/api/v1/proxy-lab'
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    def request(path='',method='GET',body=None,origin=url):
        r=urllib.request.Request(url+base+path,method=method,headers={'Origin':origin,'Content-Type':'application/json'},data=None if body is None else json.dumps(body).encode())
        try:response=opener.open(r,timeout=15)
        except urllib.error.HTTPError as e:response=e
        with response:return response.status,json.load(response)
    def db(sql):
        r=subprocess.run(['/usr/lib/postgresql/18/bin/psql','-XAt','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c',sql],capture_output=True,text=True,timeout=10)
        assert r.returncode==0,'Private proxy fixture SQL failed';return r.stdout.strip()
    status=request()[1]
    require(len(status['providers'])==8 and sum(p['api'] for p in status['providers'])==3 and status['check_seconds']==180,'eight reputation sources expose verified API capability and a three-minute platform schedule')
    yaml="proxies:\n - {name: 'US fixture', type: ss, server: 8.8.8.8, port: 443, cipher: aes-128-gcm, password: fixture-only-proxy-secret}\n - {name: '日本 fixture', type: ss, server: 1.1.1.1, port: 443, cipher: aes-128-gcm, password: fixture-only-proxy-secret}\n"
    payload={'name':'Private YAML fixture','filename':'original-provider.yaml','yaml':yaml,'enable':False}
    require(request('/profiles','POST',payload,origin='https://foreign.invalid')[0]==403,'proxy imports reject cross-origin requests')
    code,created=request('/profiles','POST',payload);assert code==200,created
    profile=created['id'];path='/profiles/'+profile
    detail=request(path)[1];node=detail['nodes'][0];node_id=node['id']
    require(len(detail['nodes'])==2 and all(n['history']=='-'*480 and n['availability_percent'] is None for n in detail['nodes']),'new histories keep all unsampled slots unknown, without fabricated uptime')
    require('fixture-only-proxy-secret' not in json.dumps(detail) and 'fixture-only-proxy-secret' not in db("SELECT encode(definition,'escape') FROM proxy_lab_nodes"),'API and database node definitions never contain the plaintext proxy password')
    require((root/'proxy-lab/master.key').stat().st_mode&0o077==0,'separate proxy master key is private to the controller account')
    require(request('/profiles','POST',payload|{'yaml':"proxies: [ fixture-must-not-echo"})[0]==400 and 'fixture-must-not-echo' not in json.dumps(request('/profiles','POST',payload|{'yaml':"proxies: [ fixture-must-not-echo"})[1]),'malformed YAML returns safe location-only errors')
    changed_yaml=yaml.replace('fixture-only-proxy-secret','changed-secret')
    require(request('/profiles','POST',payload|{'yaml':changed_yaml,'replace_id':profile,'expected_revision':1,'name':'config.yaml','filename':'config.yaml'})[0]==409,
        'a differently named import cannot replace the selected configuration or silently borrow its name')
    require(request('/profiles','POST',payload|{'yaml':changed_yaml})[0]==409 and request(path)[1]['nodes'][0]['revision']==1,
        'same-name imports require explicit overwrite and leave the original revision and nodes untouched otherwise')
    _,replacement=request('/profiles','POST',payload|{'yaml':changed_yaml,'replace_id':profile,'expected_revision':1,'filename':'config.yaml'})
    updated=request(path)[1]
    require(replacement['revision']==2 and {n['id'] for n in detail['nodes']}=={n['id'] for n in updated['nodes']},'replacing credentials preserves endpoint identities and history while advancing the configuration revision')
    require(request('/profiles','POST',payload|{'replace_id':profile,'expected_revision':1})[0]==409,'stale configuration replacements are refused')
    current=next(p for p in request()[1]['profiles'] if p['id']==profile)
    require(current['name']=='Private YAML fixture' and current['original_filename']=='original-provider.yaml','explicit same-name replacement preserves both the display name and first recorded filename')
    duplicate=request('/profiles','POST',payload|{'yaml':yaml.replace('fixture-only-proxy-secret','changed-secret'),'name':'accidental duplicate','enable':True})[1]
    require(duplicate['id']==profile and duplicate['deduplicated'] and not duplicate['enabled'] and len(request()[1]['profiles'])==1,'differently named identical content is deduplicated without overwriting the original identity, name or paused switch')
    rename={'name':'Retained provider name','expected_name':'Private YAML fixture','expected_revision':2}
    require(request(path+'/name','PUT',rename,origin='https://foreign.invalid')[0]==403 and request(path+'/name','PUT',rename)[0]==200 and request(path+'/name','PUT',rename)[0]==409,'renaming is explicit, same-origin and rejects stale editors without changing content revision')
    manual={'ip':'8.8.8.8','raw_score':20,'direction':'risk','observed_at':datetime.now(timezone.utc).isoformat(),'expected_revision':2}
    scorepath='/nodes/'+node_id+'/scores/ippure'
    require(request(scorepath,'PUT',manual)[0]==200 and request(path)[1]['nodes'][0]['scores'][0]['purity']==80,'manual risk scores retain source evidence and normalize polarity without claiming an API result')
    require(request(scorepath,'PUT',manual|{'ip':'127.0.0.1'})[0]==400 and request(scorepath,'PUT',manual|{'expected_revision':1})[0]==409,'manual evidence requires a public IP and the current configuration revision')
    require(request(path+'/score','POST',{'node_ids':[]})[0]==400 and request(path+'/score','POST',{'node_ids':[node_id,node_id]})[0]==400,'manual scoring validates batch membership before scheduling network traffic')
    require(request(path,'PUT',{'enabled':True,'expected_revision':2})[0]==200,'availability can be enabled independently of manual scoring')
    deadline=time.monotonic()+12
    while time.monotonic()<deadline:
        observed=request(path)[1]
        if all(n['observed_samples']==1 for n in observed['nodes']):break
        time.sleep(.1)
    require(all(n['latest']['latency_ms']==17 and n['availability_percent']==100 and n['observed_samples']==1 and n['coverage_percent']<1 for n in observed['nodes']),'independent Unix core records one real slot, response latency and truthful observation coverage')
    require(len(status['test_sites'])==10 and status['availability_policy']==5 and status['pass_percent_inclusive']==80 and status['retention_days']==7
        and all(n['latest']['sites']==[17]*10 and n['latest']['policy']==5 for n in observed['nodes']),
        'ten HTTPS sites contribute verified compact evidence to each node, with the inclusive 80-percent policy')
    # Keep a first profile enabled while new profiles enter the asynchronous queue.
    fresh={'name':'Second fixture','yaml':yaml+'# second config\n'}
    code,second=request('/profiles','POST',fresh);assert code==200,second
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        if second['id'] in request()[1].get('checking_profile_ids',[]):break
        time.sleep(.02)
    require(second['id'] in request()[1].get('checking_profile_ids',[]),'the API stays responsive and identifies the profile while its asynchronous core is running')
    third=request('/profiles','POST',fresh|{'name':'Third fixture','yaml':yaml+'# third config\n'})[1]
    profiles=request()[1]['profiles']
    require(len(profiles)==3 and all(p['enabled'] for p in profiles),'all imports default to enabled; importing another profile never disables an existing one')
    for item in [second,third]:
        deadline=time.monotonic()+10
        while time.monotonic()<deadline:
            detail=request('/profiles/'+item['id'])[1]
            if all(n['observed_samples']==1 for n in detail['nodes']):break
            time.sleep(.05)
        require(all(n['observed_samples']==1 for n in detail['nodes']),'each queued profile receives real observations, including the profile active when another import arrived')
    failed=request('/profiles','POST',fresh|{'name':'Failing core fixture','yaml':yaml.replace('port: 443','port: 457')})[1]
    later=request('/profiles','POST',fresh|{'name':'After failure fixture','yaml':yaml+'# after failure\n'})[1]
    deadline=time.monotonic()+10
    while time.monotonic()<deadline:
        a=request('/profiles/'+failed['id'])[1]['nodes'];b=request('/profiles/'+later['id'])[1]['nodes']
        if all(n['latest'] for n in a) and all(n['observed_samples']==1 for n in b):break
        time.sleep(.05)
    require(all(n['latest']['state']=='unknown' for n in a) and all(n['observed_samples']==1 for n in b),'a failed core is recorded as unknown and cannot block the next queued configuration')
    for item in request()[1]['profiles']:
        request('/profiles/'+item['id'],'PUT',{'enabled':False,'expected_revision':item['revision']})
    deadline=time.monotonic()+4
    while time.monotonic()<deadline and request()[1]['checking']:time.sleep(.05)
    live=set()
    for event in (root/'core-events.jsonl').read_text().splitlines():
        event=json.loads(event)
        if event['event']=='start':
            live.add(event['pid'])
        else:live.discard(event['pid'])
    require(not live,'all private fixture cores have been reaped after their batches')
    for item in [second,third,failed,later]:db(f"DELETE FROM proxy_lab_profiles WHERE id='{item['id']}'")
    require(not any(p['enabled'] for p in request()[1]['profiles']),'pausing profiles is independent and leaves no running monitoring fixture')
    exercise_merge(root,request,db,profile,node_id,path,yaml,require)
    exercise_delete(root,request,db,profile,yaml,require)
    exercise_multisite(root,request,db,require)
    exercise_duplicate_aliases(root,request,db,require)
    exercise_parallel_profiles(root,request,db,require)
    exercise_site_cooldown(root,request,db,require)
    final=request(path)[1]
    exercise_archive(root,request,db,profile,node_id,require)
    return {'profile':profile,'node':node_id,'detail':final['nodes'],'window_start':final['window_start']}

def exercise_multisite(root,request,db,require):
    yaml='proxies:\n'+''.join(f" - {{name: 'case-{port}', type: http, server: 8.8.8.8, port: {port}}}\n" for port in range(461,469))
    created=request('/profiles','POST',{'name':'Multisite fixture','yaml':yaml})[1];path='/profiles/'+created['id']
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        nodes=request(path)[1]['nodes']
        if len(nodes)==8 and all(n['latest'] for n in nodes):break
        time.sleep(.05)
    by_name={n['name']:n for n in nodes}
    states=['up','up','up','down','slow','up','down','up']
    require([by_name[f'case-{p}']['latest']['state'] for p in range(461,469)]==states,
        '8/10 succeeds, 7/10 fails, wrong HTTP status cannot pass, and misleading/opaque controller results require an independent native check')
    require(by_name['case-461']['latest']['sites']==[17]*9+[-1]
        and by_name['case-463']['latest']['sites']==[17]*8+[-1,-1]
        and by_name['case-465']['latest']['latency_ms']==1601,
        'per-site expected-status evidence overrides misleading delay responses; slow uses successful-site median')
    events=[json.loads(v) for v in (root/'site-events.jsonl').read_text().splitlines()]
    own=[e for e in events if e['node'] in {n['id'] for n in nodes}]
    require(len(own)==80 and max(e['active'] for e in own)>16 and max(e['per_node'] for e in own)>2
        and all(len({e['url'] for e in own if e['node']==n['id']})==10 for n in nodes),
        'every node tests every source once; concurrent probes exceed the former 16-global and 2-per-node caps')
    require(all(e['timeout']==['8000'] and e['expected'] in (['200'],['204']) for e in own),
        'each source uses the eight-second deadline; reconnecting a congested local Unix socket never duplicates an external probe')
    require(all(n['node_link']['latest'] and n['node_link']['history'].count('d')==1 for n in nodes),
        'direct node reachability has independent history; isolated unrouteable TCP links never inherit successful website checks')
    db(f"DELETE FROM proxy_lab_profiles WHERE id='{created['id']}'")

def exercise_duplicate_aliases(root,request,db,require):
    yaml='proxies:\n'+''.join(f" - {{name: 'same-endpoint-alias-{j}', type: http, server: 8.8.8.8, port: 443}}\n" for j in range(8))
    p=request('/profiles','POST',{'name':'Identical aliases fixture','yaml':yaml})[1]
    deadline=time.monotonic()+12
    while time.monotonic()<deadline:
        nodes=request('/profiles/'+p['id'])[1]['nodes']
        if len(nodes)==8 and all(n['latest'] for n in nodes):break
        time.sleep(.05)
    events=[json.loads(v) for v in (root/'site-events.jsonl').read_text().splitlines()]
    own=[e for e in events if e['node'] in {n['id']for n in nodes}]
    require(len(own)==10 and len({e['node']for e in own})==1 and all(n['latest']['state']=='up' and len(n['latest']['sites'])==10 and n['node_link']['latest'] and n['observed_samples']==1 for n in nodes),
        'identical aliases share one real ten-site probe, retain all eight node histories and avoid multiplying upstream connections')
    db(f"DELETE FROM proxy_lab_profiles WHERE id='{p['id']}'")

def exercise_parallel_profiles(root,request,db,require):
    profiles=[]
    for index in range(3):
        yaml='proxies:\n'+''.join(f" - {{name: 'parallel-{index}-{j}', type: http, server: 8.8.8.8, port: {443+j}}}\n" for j in range(12))
        p=request('/profiles','POST',{'name':f'Parallel fixture {index}','yaml':yaml,'enable':False})[1];profiles.append(p['id'])
    ids=','.join("'"+p+"'" for p in profiles)
    db(f"UPDATE proxy_lab_profiles SET enabled=TRUE WHERE id IN ({ids})")
    # A scoped unchanged toggle wakes the scheduler after all fixtures are committed.
    request('/profiles/'+profiles[0],'PUT',{'enabled':True,'expected_revision':1})
    started=time.monotonic();deadline=started+12
    while time.monotonic()<deadline:
        values=[request('/profiles/'+p)[1]['nodes'] for p in profiles]
        if all(len(nodes)==12 and all(n['latest'] for n in nodes) for nodes in values):break
        time.sleep(.05)
    require(all(len(nodes)==12 and all(n['latest'] for n in nodes) for nodes in values) and time.monotonic()-started<12,
        'all profiles and nodes finish the same three-minute slot in parallel, without a serial profile queue')
    require(db(f"SELECT count(DISTINCT checked_at) FROM proxy_lab_rounds WHERE profile_id IN ({ids})")=='1',
        'one fleet-wide timestamp covers all due configurations; there is no per-profile cadence drift')
    active_ids={n['id'] for nodes in values for n in nodes};events=[json.loads(v) for v in (root/'site-events.jsonl').read_text().splitlines()]
    own=[e for e in events if e['node'] in active_ids]
    require(len(own)==36*10 and max(e['active'] for e in own)>16,
        'a multi-profile batch probes every configured source for every node above the previous concurrency ceiling')
    status=request()[1]
    require(status['queue_mode']=='parallel' and status['probe_concurrency'] is None and status['round']['completed_nodes']==status['round']['expected_nodes']==status['round']['saved_nodes'] and status['round']['timed_out_nodes']==0,
        'round progress confirms actual completed and saved nodes, deadline outcomes and uncapped concurrency')
    from isolated_smoke import parse_timestamp
    boundary=parse_timestamp(status['round']['next_run_at']).timestamp()
    started_at=parse_timestamp(status['round']['started_at']).timestamp()
    require(boundary%180==0 and 0<boundary-started_at<=180,
        'the next sampling time stays on the absolute three-minute boundary rather than completion plus three minutes')
    # An upgrade must replace the current slot's old policy immediately instead
    # of waiting until the next boundary or repeatedly probing without saving.
    old=profiles[0]
    db(f"UPDATE proxy_lab_rounds SET results=(SELECT jsonb_agg(item||jsonb_build_object('policy',2)) FROM jsonb_array_elements(results) item) WHERE profile_id='{old}'")
    request('/profiles/'+old,'PUT',{'enabled':True,'expected_revision':1})
    deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        upgraded=request('/profiles/'+old)[1]['nodes']
        if all(n['latest']['policy']==5 for n in upgraded):break
        time.sleep(.05)
    require(all(n['latest']['policy']==5 for n in upgraded),
        'an old-policy current slot is replaced immediately with real new-policy probes after upgrade')
    db(f"DELETE FROM proxy_lab_profiles WHERE id IN ({ids})")

def exercise_site_cooldown(root,request,db,require):
    yaml="proxies:\n - {name: 'cooldown fixture', type: http, server: 8.8.8.8, port: 443}\n"
    identity=request('/profiles','POST',{'name':'Cooldown fixture','yaml':yaml,'enable':False})[1]['id'];path='/profiles/'+identity
    node=request(path)[1]['nodes'][0]['id']
    db(f"INSERT INTO proxy_lab_rounds(profile_id,slot,checked_at,revision,results) SELECT '{identity}',date_bin(INTERVAL '180 seconds',NOW(),TIMESTAMPTZ '1970-01-01')-INTERVAL '180 seconds',NOW()-INTERVAL '180 seconds',1,jsonb_build_array(jsonb_build_object('id','{node}','state','unknown','latency_ms',NULL,'detail','fixture cooldown','policy',4,'sites',jsonb_build_array(-2,17,17,17,17,17,17,17,17,17),'site_feedback',jsonb_build_array(jsonb_build_object('site',0,'primary_status',429,'fallback_status',429,'retry_at',NOW()+INTERVAL '1 hour','fallback_retry_at',NOW()+INTERVAL '1 hour','rate_limited',TRUE,'outcome','rate_limited'))))")
    request(path,'PUT',{'enabled':True,'expected_revision':1});deadline=time.monotonic()+8
    while time.monotonic()<deadline:
        value=request(path)[1]['nodes'][0]
        if value['latest']['detail']!='fixture cooldown':break
        time.sleep(.05)
    evidence=value['latest']['site_feedback'][0]
    require(value['latest']['sites']==[-2]+[17]*9 and evidence['rate_limited'] and evidence['primary_status']==429,
        'persisted Retry-After cooldown survives the policy-4 to policy-5 upgrade without repeating cooling origins')
    own=[json.loads(v) for v in (root/'site-events.jsonl').read_text().splitlines() if json.loads(v)['node']==node]
    require(len(own)==9 and all('www.google.com/' not in e['url'] for e in own),
        'cooling primary and backup origins receive no repeated requests; other service votes remain independent')
    db(f"DELETE FROM proxy_lab_profiles WHERE id='{identity}'")

def exercise_archive(root,request,db,profile,node_id,require):
    # Reuse the same compact round table. Old policy 4 keeps its original state,
    # while current policy 5 accepts 8/10; no historical observation is rewritten.
    baseline=next(n for n in request('/profiles/'+profile)[1]['nodes'] if n['id']==node_id)
    baseline_down=baseline['history'].count('d');baseline_link_up=baseline['node_link']['history'].count('u')
    for age in [3,6,8]:
        db(f"INSERT INTO proxy_lab_rounds(profile_id,slot,checked_at,revision,results) SELECT '{profile}',date_bin(INTERVAL '180 seconds',NOW(),TIMESTAMPTZ '1970-01-01')-INTERVAL '{age} days',NOW()-INTERVAL '{age} days',2,jsonb_build_array(jsonb_build_object('id','{node_id}','state','down','latency_ms',NULL,'detail','archive fixture {age}','policy',4,'sites',jsonb_build_array(17,17,17,17,17,17,17,17,-1,-1),'link',jsonb_build_object('state','up','latency_ms',5,'detail','TCP connected','kind','tcp')))")
    url=f'/profiles/{profile}/history?node={node_id}&days=7'
    before=len((root/'site-events.jsonl').read_text().splitlines())
    code,archive=request(url);assert code==200,archive
    require(code==200 and archive['days']==7 and archive['retention_days']==7 and len(archive['https']['history'])==3360 and archive['https']['history'].count('d')==baseline_down+2 and archive['node_link']['history'].count('u')==baseline_link_up+2,
        'seven-day history reads old per-node records and keeps original policy outcomes, independent TCP data and real missing gaps')
    logs=f'/profiles/{profile}/logs?node={node_id}&days=7&limit=1'
    _,first=request(logs);require(first['has_more'] and len(first['entries'])==1 and first['next_before'],
        'historical test logs paginate directly from existing rounds with a bounded cursor')
    _,second=request(logs+'&before='+first['next_before']);require(second['entries'] and second['entries'][0]['slot']<first['entries'][0]['slot'] and 'definition' not in json.dumps(first) and 'fixture-only-proxy-secret' not in json.dumps(archive),
        'cursor pagination has no repeated round and history responses exclude proxy definitions and secrets')
    _,daily=request(f'/profiles/{profile}/history?node={node_id}&days=1')
    require(len(daily['https']['history'])==480 and daily['https']['history'].count('d')==baseline_down,
        '24-hour monitoring stays limited to 480 slots while the separate seven-day read is requested explicitly')
    require(request(f'/profiles/{profile}/history?node={node_id}&days=30')[0]==400 and request(logs.replace('limit=1','limit=101'))[0]==400,
        'history scope and log page sizes cannot exceed the seven-day and 100-round bounds')
    other=request('/profiles','POST',{'name':'Wrong-owner fixture','yaml':"proxies: [{name: foreign, type: http, server: 8.8.8.8, port: 443}]",'enable':False})[1]['id']
    require(request(f'/profiles/{other}/history?node={node_id}&days=7')[0]==404 and request(f'/profiles/{other}/logs?node={node_id}')[0]==404,
        'history and logs validate node ownership before reading another configuration')
    db(f"DELETE FROM proxy_lab_profiles WHERE id='{other}'")
    require(len((root/'site-events.jsonl').read_text().splitlines())==before,
        'opening history and paging logs never launches additional outbound probes')

def verify(url,context,saved,require):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    with opener.open(url+'/api/v1/proxy-lab/profiles/'+saved['profile'],timeout=5) as response:result=json.load(response)
    def samples(node,start):
        epoch=datetime.fromisoformat(start.replace('Z','+00:00')).timestamp()
        return {epoch+i*180:state for i,state in enumerate(node['history']) if state!='-'}
    def stable(node):
        return {k:({a:b for a,b in v.items() if a!='history'} if k=='node_link' else v) for k,v in node.items() if k!='history'}
    observed=result['nodes']
    equal=len(observed)==len(saved['detail']) and all(
        stable(now)==stable(before)
        and samples(now,result['window_start'])==samples(before,saved['window_start'])
        and samples(now['node_link'],result['window_start'])==samples(before['node_link'],saved['window_start'])
        for now,before in zip(observed,saved['detail']))
    until=time.monotonic()+4
    while time.monotonic()<until:
        result=subprocess.run(['/usr/lib/postgresql/18/bin/psql','-XAt','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-c',"SELECT COUNT(*) FROM proxy_lab_rounds WHERE slot<NOW()-INTERVAL '7 days'"],capture_output=True,text=True,timeout=5)
        if result.returncode==0 and result.stdout.strip()=='0':break
        time.sleep(.05)
    require(result.returncode==0 and result.stdout.strip()=='0','automatic seven-day retention removes expired test rounds after platform restart without deleting recent history')
    with opener.open(url+'/api/v1/proxy-lab/profiles/'+saved['profile']+'/logs?node='+saved['node']+'&days=7&limit=100',timeout=5) as response:archived=json.load(response)
    retained={entry['observation']['detail'] for entry in archived['entries']}
    require({'archive fixture 3','archive fixture 6'}.issubset(retained) and 'archive fixture 8' not in retained,'automatic retention preserves the third and sixth days while expiring the eighth day')
    require(equal,'proxy ciphertext identity, score evidence and timestamped availability history survive platform/database restarts')

def exercise_merge(root,request,db,profile,node_id,path,yaml,require):
    # Create an independently encrypted duplicate fixture, then give it the same
    # content hash to exercise merging data left by the former import policy.
    retained_ids={n['id'] for n in request(path)[1]['nodes']}
    duplicate=request('/profiles','POST',{'name':'Old duplicate','yaml':yaml.replace('fixture-only-proxy-secret','changed-secret')+'# historical duplicate\n','enable':False})[1]
    duplicate_id=duplicate['id'];nodes=request('/profiles/'+duplicate_id)[1]['nodes']
    donor=next(n for n in nodes if n['name']==request(path)[1]['nodes'][0]['name'])
    evidence={'ip':'8.8.8.8','raw_score':10,'direction':'risk','observed_at':datetime.now(timezone.utc).isoformat(),'expected_revision':1}
    require(request('/nodes/'+donor['id']+'/scores/ippure','PUT',evidence)[0]==200,'duplicate fixture has separately keyed reputation evidence')
    db(f"UPDATE proxy_lab_profiles SET content_hash=(SELECT content_hash FROM proxy_lab_profiles WHERE id='{profile}') WHERE id='{duplicate_id}'")
    for shift in [0,180]:
        db(f"INSERT INTO proxy_lab_rounds(profile_id,slot,checked_at,revision,results) SELECT '{duplicate_id}',r.slot-INTERVAL '{shift} seconds',NOW(),1,(SELECT jsonb_agg(item||jsonb_build_object('id',d.id,'state','down')) FROM jsonb_array_elements(r.results) item JOIN proxy_lab_nodes k ON k.id=(item->>'id')::UUID JOIN proxy_lab_nodes d ON d.identity_hash=k.identity_hash AND d.profile_id='{duplicate_id}') FROM proxy_lab_rounds r WHERE r.profile_id='{profile}' ORDER BY slot DESC LIMIT 1")
    from pathlib import Path
    merger=Path(__file__).resolve().parents[2]/'deploy/database/merge-identical-proxy-profiles.sql'
    command=['/usr/lib/postgresql/18/bin/psql','-Xq','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-v','keep_id='+profile,'-v','drop_id='+duplicate_id,'-v','keep_name=Merged provider','-f',str(merger)]
    result=subprocess.run(command,capture_output=True,text=True,timeout=20)
    assert result.returncode==0,result.stderr
    merged=request(path)[1]['nodes']
    require({n['id'] for n in merged}==retained_ids and all(n['observed_samples']==2 for n in merged),'merging preserves the retained node identities and unions duplicate history slots')
    require(merged[0]['id']==node_id and merged[0]['scores'][0]['purity']==90 and merged[0]['scores'][0]['revision']==2,'the latest duplicate score is transferred with the retained node revision')
    require(len(request()[1]['profiles'])==1 and request()[1]['profiles'][0]['name']=='Merged provider','only the duplicate profile is deleted after history and evidence have been merged')

def rollback(server,current,previous,root,env,url,context,log,require):
    """Only on the smoke runner's private schema/database and processes."""
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    def start(binary):
        p=subprocess.Popen([str(binary)],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
        for _ in range(100):
            try:
                with opener.open(url+'/api/v1/health/ready',timeout=1) as r:
                    if r.status==200:return p
            except OSError:time.sleep(.1)
        p.terminate();p.wait(timeout=10);raise AssertionError('Private rollback fixture not ready')
    server.terminate();server.wait(timeout=20)
    script=__import__('pathlib').Path(__file__).resolve().parents[2]/'deploy/database/rollback-proxy-lab.sql'
    queue=script.with_name('rollback-proxy-lab-queue.sql')
    local=script.with_name('rollback-local-network.sql')
    monitoring=script.with_name('rollback-proxy-monitoring.sql')
    subprocess.run(['/usr/lib/postgresql/18/bin/psql','-Xq','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-f',str(monitoring)],stdout=log,stderr=log,check=True,timeout=10)
    subprocess.run(['/usr/lib/postgresql/18/bin/psql','-Xq','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-f',str(local)],stdout=log,stderr=log,check=True,timeout=10)
    subprocess.run(['/usr/lib/postgresql/18/bin/psql','-Xq','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-f',str(queue)],stdout=log,stderr=log,check=True,timeout=10)
    subprocess.run(['/usr/lib/postgresql/18/bin/psql','-Xq','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-f',str(script)],stdout=log,stderr=log,check=True,timeout=10)
    old=start(previous)
    try:
        with opener.open(url+'/api/v1/nodes',timeout=5) as r:require(r.status==200,'proxy queue rollback preserves monitoring on the previous platform binary')
    finally:old.terminate();old.wait(timeout=20)
    return start(current)

def exercise_delete(root,request,db,retained,yaml,require):
    original=root/'delete-original.yaml';original.write_text(yaml);original.chmod(0o600)
    before=request()[1]['profiles'];retained_before=request('/profiles/'+retained)[1]['nodes']
    imported=request('/profiles','POST',{'name':'Delete fixture','filename':original.name,'yaml':yaml.replace('port: 443','port: 458'),'enable':False})[1]
    identity=imported['id'];path='/profiles/'+identity;nodes=request(path)[1]['nodes'];ids=[n['id'] for n in nodes]
    confirmation={'expected_name':'Delete fixture','expected_revision':1,'confirmation':'Delete fixture'}
    evidence={'ip':'8.8.8.8','raw_score':12,'direction':'risk','observed_at':datetime.now(timezone.utc).isoformat(),'expected_revision':1}
    assert request('/nodes/'+ids[0]+'/scores/ippure','PUT',evidence)[0]==200
    require(request(path,'DELETE',confirmation,origin='https://foreign.invalid')[0]==403
        and request(path,'DELETE',confirmation|{'confirmation':'wrong'})[0]==400
        and request(path,'DELETE',confirmation|{'expected_revision':0})[0]==409
        and request(path,'DELETE',confirmation|{'expected_name':'stale name'})[0]==409,
        'deletion rejects cross-origin, incorrect confirmation and stale metadata before removing data')
    assert request(path,'PUT',{'enabled':True,'expected_revision':1})[0]==200
    deadline=time.monotonic()+5
    while time.monotonic()<deadline:
        events=[json.loads(v) for v in (root/'core-events.jsonl').read_text().splitlines()]
        if any(e['event']=='start' and set(e['nodes'])==set(ids) for e in events):break
        time.sleep(.02)
    require(any(e['event']=='start' and set(e['nodes'])==set(ids) for e in events),'delete fixture begins a real asynchronous availability batch')
    assert request(path,'DELETE',confirmation)[0]==200
    deadline=time.monotonic()+3
    while time.monotonic()<deadline:
        events=[json.loads(v) for v in (root/'core-events.jsonl').read_text().splitlines()]
        matching=[e for e in events if set(e['nodes'])==set(ids)]
        if any(e['event']=='stop' for e in matching) and request()[1]['checking_profile_id']!=identity:break
        time.sleep(.03)
    require(any(e['event']=='stop' for e in matching),'deleting an in-flight configuration cancels and terminates its isolated probe core')
    time.sleep(.35)
    require(request(path)[0]==404 and request(path,'DELETE',confirmation)[0]==404
        and db(f"SELECT COUNT(*) FROM proxy_lab_nodes WHERE profile_id='{identity}'")=='0'
        and db(f"SELECT COUNT(*) FROM proxy_lab_rounds WHERE profile_id='{identity}'")=='0'
        and db("SELECT COUNT(*) FROM proxy_lab_scores WHERE node_id IN("+','.join("'"+i+"'" for i in ids)+")")=='0',
        'profile deletion atomically removes only its encrypted nodes, history and scores without later resurrection')
    require(original.read_text()==yaml and [{k:v for k,v in n.items() if k!='history'} for n in request('/profiles/'+retained)[1]['nodes']]==[{k:v for k,v in n.items() if k!='history'} for n in retained_before],
        'deleting the platform copy leaves source files and other configurations unchanged')
    # Delete while an explicit reputation job is selecting its isolated proxy.
    scored=request('/profiles','POST',{'name':'Delete score fixture','yaml':yaml.replace('port: 443','port: 459'),'enable':False})[1]
    score_id=scored['id'];score_nodes=request('/profiles/'+score_id)[1]['nodes'];score_ids=[n['id'] for n in score_nodes]
    code,job=request('/profiles/'+score_id+'/score','POST',{'node_ids':score_ids});assert code==200,job
    deadline=time.monotonic()+5
    while time.monotonic()<deadline:
        events=[json.loads(v) for v in (root/'core-events.jsonl').read_text().splitlines()]
        if any(e['event']=='start' and set(e['nodes'])==set(score_ids) for e in events):break
        time.sleep(.02)
    require(any(e['event']=='start' and set(e['nodes'])==set(score_ids) for e in events),'a manual reputation job starts only after the explicit scoring request')
    body={'expected_name':'Delete score fixture','expected_revision':1,'confirmation':'Delete score fixture'}
    assert request('/profiles/'+score_id,'DELETE',body)[0]==200
    deadline=time.monotonic()+4
    while time.monotonic()<deadline:
        state=request('/jobs/'+job['job_id'])[1]
        if state['status']=='cancelled' and not request()[1]['score_busy']:break
        time.sleep(.05)
    require(state['status']=='cancelled' and not request()[1]['score_busy'],'deleting a configuration cancels its manual IP job and releases the scoring slot')
    require(request('/profiles/'+score_id+'/score','POST',{'node_ids':score_ids})[0]==400,'removed node identities cannot start new network tasks')
    reimport=request('/profiles','POST',{'name':'Fresh identity','yaml':yaml.replace('port: 443','port: 459'),'enable':False})[1]
    require(reimport['id']!=score_id and all(n['observed_samples']==0 for n in request('/profiles/'+reimport['id'])[1]['nodes']),'reimporting a deleted file creates a new identity and never resurrects deleted history')
    assert request('/profiles/'+reimport['id'],'DELETE',{'expected_name':'Fresh identity','expected_revision':1,'confirmation':'Fresh identity'})[0]==200
    require({p['id'] for p in request()[1]['profiles']}=={p['id'] for p in before},'only explicitly confirmed deletion fixtures were removed')
