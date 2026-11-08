"""Persistent probe switches; only invoked by the private namespace runner."""
import json,time,urllib.request,urllib.error

def client(url,context):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    def request(path='',method='GET',body=None,origin=None):
        req=urllib.request.Request(url+'/api/v1/proxy-lab'+path,method=method,
            headers={'Content-Type':'application/json','Origin':origin or url},data=None if body is None else json.dumps(body).encode())
        try:r=opener.open(req,timeout=15)
        except urllib.error.HTTPError as error:r=error
        with r:return r.status,json.load(r)
    return request

def until(predicate,description,seconds=8):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if predicate():return
        time.sleep(.025)
    raise AssertionError(description)

def exercise(root,url,context,require):
    request=client(url,context);original=request()[1];settings=original['monitoring']
    require(settings['enabled']and settings['local_enabled'],'network probe switches default on without changing per-profile preferences')
    path='/monitoring';body={'enabled':False,'expected_revision':settings['revision']}
    require(request(path,'PUT',body)[0]==400 and request(path,'PUT',body|{'confirmed':True},origin='https://foreign.invalid')[0]==403
        and request(path,'PUT',body|{'confirmed':True,'expected_revision':0})[0]==409 and request()[1]['monitoring']==settings,
        'master pause requires explicit confirmation, same origin and the current control revision')
    name='Pause control fixture'
    yaml="proxies:\n - {name: 'US pause fixture', type: ss, server: 8.8.8.8, port: 458, cipher: aes-128-gcm, password: isolated-fixture-only}\n"
    code,created=request('/profiles','POST',{'name':name,'yaml':yaml});assert code==200,created
    identity=created['id'];route='/profiles/'+identity
    node=request(route)[1]['nodes'][0]['id']
    def probes():
        file=root/'site-events.jsonl'
        return sum(json.loads(line).get('node')==node for line in file.read_text().splitlines())if file.exists()else 0
    until(lambda:probes()>0,'Private slow probe did not start')
    require(request(route,'PUT',{'enabled':False,'expected_revision':1,'expected_enabled':False})[0]==409,
        'stale individual switch state cannot overwrite a concurrent operator choice')
    assert request(route,'PUT',{'enabled':False,'expected_revision':1,'expected_enabled':True})[0]==200
    until(lambda:identity not in request()[1]['checking_profile_ids'],'Individual pause did not cancel active requests')
    count=probes();time.sleep(.15)
    require(probes()==count and not next(p for p in request()[1]['profiles']if p['id']==identity)['enabled']
        and request()[1]['monitoring']['enabled'],'individual pause cancels its active batch while leaving the master and other configurations enabled')
    assert request(route,'PUT',{'enabled':True,'expected_revision':1,'expected_enabled':False})[0]==200
    until(lambda:probes()>count,'Individual resume did not restart real checks')
    settings=request()[1]['monitoring']
    code,changed=request('/local/monitoring','PUT',{'enabled':False,'expected_revision':settings['revision']});assert code==200,changed
    settings=changed['monitoring']
    require(settings['enabled']and not settings['local_enabled']and identity in request()[1]['checking_profile_ids'],
        'the local website source can be disabled independently without cancelling a running proxy batch')
    choices={p['id']:p['enabled']for p in request()[1]['profiles']}
    code,paused=request(path,'PUT',{'enabled':False,'expected_revision':settings['revision'],'confirmed':True});assert code==200,paused
    until(lambda:not request()[1]['checking_profile_ids'],'Master pause did not cancel in-flight batches')
    count=probes();time.sleep(.15);status=request()[1]
    require(probes()==count and not status['monitoring']['enabled']and {p['id']:p['enabled']for p in status['profiles']}==choices,
        'master pause stops active requests and preserves every individual choice instead of rewriting all profiles')
    require(request(route+'/score','POST',{'node_ids':[node]})[0]==409,
        'the master switch also blocks new manual network scoring while paused')
    _,local=request('/local')
    require(not local['enabled']and not local['global_enabled']and not local['configured_enabled'],
        'local source reports effective pause separately from its persisted individual choice')
    second_name='Imported during pause'
    code,second=request('/profiles','POST',{'name':second_name,'yaml':yaml.replace('458','459')});assert code==200,second
    time.sleep(.15)
    require(not request()[1]['checking_profile_ids']and any(p['id']==second['id']and p['enabled']for p in request()[1]['profiles']),
        'new YAML imports remain configured but cannot bypass the paused master switch')
    # Remove only these explicitly created fixtures. Keep the master/local off
    # until the final cold restart to verify persistence without touching hosts.
    for item,title in [(identity,name),(second['id'],second_name)]:
        assert request('/profiles/'+item,'DELETE',{'expected_name':title,'expected_revision':1,'confirmation':title})[0]==200
    return {'original':settings|{'enabled':original['monitoring']['enabled'],'local_enabled':original['monitoring']['local_enabled']},
        'paused':request()[1]['monitoring'],'choices':{p['id']:p['enabled']for p in original['profiles']}}

def verify(url,context,saved,require):
    request=client(url,context);status=request()[1]
    require(status['monitoring']==saved['paused']and not status['monitoring']['enabled']
        and not status['monitoring']['local_enabled']and {p['id']:p['enabled']for p in status['profiles']}==saved['choices'],
        'master/local pause and original per-profile choices survive real database/platform restarts')
    settings=status['monitoring']
    _,updated=request('/local/monitoring','PUT',{'enabled':saved['original']['local_enabled'],'expected_revision':settings['revision']})
    settings=updated['monitoring']
    require(request('/monitoring','PUT',{'enabled':True,'expected_revision':settings['revision']})[0]==400,
        'resuming all checks requires a second confirmation just like pausing them')
    code,resumed=request('/monitoring','PUT',{'enabled':saved['original']['enabled'],'expected_revision':settings['revision'],'confirmed':True})
    require(code==200 and resumed['monitoring']['enabled']==saved['original']['enabled']
        and {p['id']:p['enabled']for p in request()[1]['profiles']}==saved['choices'],
        'confirmed resume restores the master while respecting independent per-profile preferences')
