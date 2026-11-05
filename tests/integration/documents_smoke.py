"""Editable Markdown on the parent test's private HTTPS/PostgreSQL only."""
import concurrent.futures
import json
import urllib.error
import urllib.request

def exercise(url, context, require):
    path='/api/v1/documents/gpu-comparison'
    def request(method='GET', payload=None, headers=None, suffix=''):
        opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
        req=urllib.request.Request(url+path+suffix,method=method,
            data=None if payload is None else json.dumps(payload).encode(),
            headers={'Content-Type':'application/json','Origin':url}|(headers or {}))
        try:response=opener.open(req,timeout=8)
        except urllib.error.HTTPError as error:response=error
        with response:return response.status,response.headers,response.read()
    code,headers,body=request()
    initial=json.loads(body)
    require(code==200 and initial['revision']==0 and 'Tesla V100' in initial['content'] and headers['Cache-Control']=='no-store',
            'Markdown reference initially serves the supplied document without creating a monitoring sample')
    for payload,extra,expected in [({'content':'# forbidden','expected_revision':0},{'Origin':'https://other.invalid'},403),
        ({'content':'binary\0content','expected_revision':0},{},400),
        ({'content':'x'*1048577,'expected_revision':0},{},400),
        ({'content':'# no path','expected_revision':0,'path':'/tmp/escape'},{},422),
        ({'content':'# no revision','expected_revision':-1},{},400)]:
        result=request('PUT',payload,extra)
        require(result[0]==expected,'document writes reject cross-origin, binary, oversized, path and invalid revision requests')
    require(json.loads(request()[2])==initial,'rejected document writes leave the original document intact')
    def write(content):return request('PUT',{'content':content,'expected_revision':0,'source':'edit'})
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        results=list(pool.map(write,['# first editor\n','第二位编辑者\n']))
    require(sorted(result[0] for result in results)==[200,409], 'concurrent first edits commit exactly one version and reject the stale editor')
    winner=json.loads(next(result[2] for result in results if result[0]==200))
    require(winner['revision']==1 and winner['updated_at'] is not None, 'shared document save has a durable revision and timestamp')
    require(request('PUT',{'content':'# stale upload','expected_revision':0,'source':'upload'})[0]==409
            and json.loads(request()[2])['content']==winner['content'], 'stale uploads cannot silently overwrite another editor')
    uploaded='# 上传后的 GPU 对比\r\n\r\n| GPU | 说明 |\r\n|---|---|\r\n| V100 | 保留 Markdown |\r\n'
    code,_,body=request('PUT',{'content':uploaded,'expected_revision':1,'source':'upload'})
    saved=json.loads(body)
    require(code==200 and saved['content']==uploaded and saved['revision']==2, 'Markdown upload replaces the shared document while preserving Unicode and line endings')
    code,headers,body=request(suffix='/download')
    require(code==200 and body==uploaded.encode() and headers['Content-Type'].startswith('text/markdown')
            and "filename*=UTF-8''" in headers['Content-Disposition'] and headers['X-Content-Type-Options']=='nosniff',
            'Markdown download returns the exact saved UTF-8 source as an attachment')
    return saved
