"""External shortcuts on the parent test's private HTTPS/PostgreSQL only."""
import concurrent.futures
import json
import urllib.error
import urllib.request
import uuid


def exercise(url, context, require):
    endpoint = '/api/v1/external-platforms'

    def request(method='GET', payload=None, path=endpoint, headers=None):
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
        query = urllib.request.Request(url + path, method=method,
            data=None if payload is None else json.dumps(payload).encode(),
            headers={'Content-Type': 'application/json', 'Origin': url} | (headers or {}))
        try:
            response = opener.open(query, timeout=8)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read()
            return response.status, json.loads(raw) if response.headers.get_content_type() == 'application/json' else raw.decode()

    def outbound_started():
        return {item['group']: item['started'] for item in request(path='/api/v1/platform-network')[1]['server']['groups']
                if item['group'] != 'http_browser'}

    before_nodes = {item['id'] for item in request(path='/api/v1/nodes')[1]['nodes']}
    before_outbound = outbound_started()
    require(request()[1]['platforms'] == [], 'external shortcut list starts independently of managed nodes')
    fixture = {'name': ' 算力平台 ', 'url': 'https://compute.example/gpu?region=lab#login', 'note': ' shared shortcut '}
    require(request('POST', fixture, headers={'Origin': 'https://foreign.invalid'})[0] == 403,
            'cross-origin shortcut creation is rejected without probing the destination')
    for invalid in [fixture | {'name': ' '}, fixture | {'url': 'javascript:alert(1)'},
                    fixture | {'url': 'file:///tmp/a'}, fixture | {'url': '//compute.example'},
                    fixture | {'url': 'https://user:password@compute.example'}, fixture | {'url': 'https://good\n.example'}]:
        require(request('POST', invalid)[0] == 400, 'shortcut metadata rejects unsafe destinations and empty names')
    require(request('POST', fixture | {'ssh_password': 'fixture-not-a-real-password'})[0] == 422,
            'external shortcuts cannot accept Agent or SSH credentials')
    code, created = request('POST', fixture)
    require(code == 200 and created['name'] == '算力平台' and created['note'] == 'shared shortcut'
            and created['revision'] == 1 and created['deleted_at'] is None,
            'shortcut creation saves shared metadata with its own identity and revision')
    path = endpoint + '/' + created['id']

    def edit(name):
        return request('PUT', fixture | {'name': name, 'expected_revision': 1}, path=path)

    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
        edits = list(pool.map(edit, ['first editor', 'second editor']))
    require(sorted(item[0] for item in edits) == [200, 409], 'concurrent shortcut edits reject a stale editor instead of losing changes')
    edited = next(item[1] for item in edits if item[0] == 200)
    manage = path + '/manage'
    change = {'action': 'delete', 'confirmation': created['id'], 'expected_revision': 2}
    require(request('POST', change | {'confirmation': str(uuid.uuid4())}, path=manage)[0] == 400,
            'soft deletion requires the exact external shortcut identity')
    code, archived = request('POST', change, path=manage)
    require(code == 200 and archived['revision'] == 3 and archived['deleted_at'] is not None,
            'soft deletion retains metadata and identity in the shared archive')
    require(request('PUT', fixture | {'expected_revision': 3}, path=path)[0] == 409,
            'archived shortcuts cannot be edited without restoring them')
    code, restored = request('POST', change | {'action': 'restore', 'expected_revision': 3}, path=manage)
    require(code == 200 and restored['revision'] == 4 and restored['deleted_at'] is None
            and restored['url'] == edited['url'] and restored['id'] == created['id'],
            'restoring a shortcut retains its original link and identity')
    require(request('POST', change, path=manage)[0] == 409, 'stale shortcut actions cannot overwrite restored state')
    code, internal = request('POST', {'name': 'internal HTTP fixture', 'url': 'http://192.0.2.10:8080/console'})
    require(code == 200, 'intranet HTTP destinations can be saved without a deployed Agent or CA')
    _, archived = request('POST', {'action': 'delete', 'confirmation': internal['id'], 'expected_revision': 1},
                          path=endpoint + '/' + internal['id'] + '/manage')
    require({item['id'] for item in request(path='/api/v1/nodes')[1]['nodes']} == before_nodes
            and outbound_started() == before_outbound,
            'shortcut lifecycle never creates managed nodes or initiates HTTP, TCP, SSH or destination probes')
    observed = {item['id']: item for item in request()[1]['platforms']}
    require(observed == {restored['id']: restored, archived['id']: archived},
            'the platform shares both active and soft-deleted shortcuts across clients')
    return observed
