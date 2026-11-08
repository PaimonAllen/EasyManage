"""Direct-network storage/read fixtures; only called inside the private namespace."""
import json,subprocess,time,urllib.request,urllib.error

def request(url,context,path):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    try:response=opener.open(url+'/api/v1/proxy-lab/local'+path,timeout=15)
    except urllib.error.HTTPError as error:response=error
    with response:return response.status,json.load(response)

def db(sql):
    command=['/usr/lib/postgresql/18/bin/psql','-XAt','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c',sql]
    result=subprocess.run(command,capture_output=True,text=True,timeout=10)
    assert result.returncode==0,'Private local-network SQL fixture failed'
    return result.stdout.strip()

def exercise(url,context,require):
    code,data=request(url,context,'');sites=data['nodes'];ids={s['id']for s in sites}
    require(code==200 and data['source']=='local_network'and len(sites)==20 and len(ids)==20 and all(s['url'].startswith('https://')and len(s['history'])==480 for s in sites),
        'local-network source exposes twenty static HTTPS websites with independent 24-hour histories')
    node=sites[0]['id']
    results=[{'id':s['id'],'state':'up','latency_ms':25,'http_status':403 if i==0 else 200,'detail':'','policy':1}for i,s in enumerate(sites)]
    results[1].update(policy=2,http_fallback={'head_status':405,'get_status':200})
    for age in ['15 minutes','3 days','8 days']:
        db("INSERT INTO local_network_rounds(slot,checked_at,results) VALUES(date_bin(INTERVAL '180 seconds',NOW(),TIMESTAMPTZ '1970-01-01')-INTERVAL '"+age+"',NOW()-INTERVAL '"+age+"', '"+json.dumps(results)+"'::JSONB)")
    code,week=request(url,context,'/history?node='+node+'&days=7')
    require(code==200 and week['source']=='local_network'and len(week['https']['history'])==3360 and week['https']['history'].count('u')==2 and week['node_link']is None,
        'local seven-day history keeps the third day, excludes the eighth day and never substitutes proxy/TCP results')
    code,logs=request(url,context,'/logs?node='+node+'&days=7&limit=1');cursor=logs['next_before']
    require(code==200 and len(logs['entries'])==1 and logs['has_more']and cursor,
        'website logs paginate directly from compact local-network rounds')
    _,older=request(url,context,'/logs?node='+node+'&days=7&limit=1&before='+cursor)
    require(older['entries'][0]['slot']<logs['entries'][0]['slot'], 'local-network history cursor does not duplicate rounds')
    _,all_logs=request(url,context,'/logs?node='+node+'&days=7&limit=100')
    fixtures=[e for e in all_logs['entries']if e['observation'].get('http_status')==403]
    require(len(fixtures)==2 and all(e['observation']['state']=='up'for e in fixtures),
        'HTTP 403 is retained as a website restriction while verified HTTPS network reachability remains separate')
    _,compatible=request(url,context,'/logs?node='+sites[1]['id']+'&days=7&limit=100')
    require(len(compatible['entries'])==2 and all(e['observation'].get('http_fallback')=={'head_status':405,'get_status':200}and e['observation']['http_status']==200 for e in compatible['entries']),
        'GET compatibility logs retain the real final status and original HEAD response without copying website bodies')
    require(request(url,context,'/history?node=00000000-0000-0000-0000-000000000001&days=7')[0]==404
        and request(url,context,'/history?days=30')[0]==400 and request(url,context,'/logs?node='+node+'&limit=101')[0]==400,
        'direct-network history accepts only its website catalog and bounded seven-day pages')
    _,proxy_status=request_proxy(url,context)
    require(all(p['id']not in ids for p in proxy_status['profiles']), 'local websites never create synthetic proxy profiles or consume imported YAML slots')
    return node

def request_proxy(url,context):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    with opener.open(url+'/api/v1/proxy-lab',timeout=10)as r:return r.status,json.load(r)

def verify(url,context,node,require):
    expired=db("SELECT COUNT(*) FROM local_network_rounds WHERE slot<NOW()-INTERVAL '7 days'")
    retained=db("SELECT COUNT(*) FROM local_network_rounds WHERE slot>=NOW()-INTERVAL '7 days'AND slot<NOW()-INTERVAL '2 days'")
    require(expired=='0' and retained=='1','local-network retention expires eight-day results and preserves three-day history through restart')
    code,logs=request(url,context,'/logs?node='+node+'&days=7&limit=100')
    require(code==200 and len([e for e in logs['entries']if e['observation'].get('http_status')==403])==2,
        'website status evidence persists across real platform and PostgreSQL restarts')
    _,catalog=request(url,context,'')
    _,compatible=request(url,context,'/logs?node='+catalog['nodes'][1]['id']+'&days=7&limit=100')
    require(len([e for e in compatible['entries']if e['observation'].get('http_fallback')=={'head_status':405,'get_status':200}])==2,
        'structured HEAD/GET compatibility evidence survives real platform/database restarts alongside older HEAD-only records')

def verify_sampler(url,context,require):
    deadline=time.monotonic()+115
    while time.monotonic()<deadline:
        _,data=request(url,context,'')
        if data['round']['saved_sites']==20:
            break
        time.sleep(.2)
    else:
        raise AssertionError('Real private-network local sampler did not save all twenty outcomes')
    require(data['enabled'] and data['round']['completed_sites']==20 and not data['round']['error'],
        'independent real local collector asynchronously completes and saves all twenty website outcomes')
    observations=[s['latest']for s in data['nodes']]
    require(all(o and o['state']=='down'and o.get('http_status')is None for o in observations),
        'isolated network has no external route: the real sampler records failures without inventing website HTTP responses')
    require(db("SELECT jsonb_array_length(results) FROM local_network_rounds ORDER BY slot DESC LIMIT 1")=='20',
        'one compact round stores all twenty observations without proxy or Agent records')
