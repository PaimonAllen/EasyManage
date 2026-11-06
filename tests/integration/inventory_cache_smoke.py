"""Cold inventory cache compatibility and failure paths in the private HTTP fixture."""
import copy
import json

FIELDS = (
    'block_devices', 'hardware_details', 'users', 'interfaces', 'overlay_networks',
    'routes', 'proxy_services', 'inventory_collected_at', 'running_services',
    'host_security', 'permission_audit', 'ai_tools', 'programming_tools',
)


def exercise(request, require, report, token, accepted):
    headers = {'x-easymanage-agent-token': token}
    key = accepted['inventory_cached']
    before = request('/api/v1/nodes')[1]['nodes'][0]['metrics']
    compact = copy.deepcopy(report)
    compact.update(protocol_version=3, reuse_inventory=key, reuse_software_inventory=True)
    compact['metrics'].pop('software_packages', None)
    for field in FIELDS:
        compact['metrics'].pop(field, None)
    status, response = request('/api/v1/agent/report', compact, headers)
    after = request('/api/v1/nodes')[1]['nodes'][0]['metrics']
    require(status == 200 and response['inventory_cached'] == key
            and before == after and len(json.dumps(compact)) < len(json.dumps(report)),
            'compact inventory keeps the full public snapshot, privileges and timestamps unchanged')
    wrong = copy.deepcopy(compact)
    wrong['reuse_inventory'] = '0' * 64
    require(request('/api/v1/agent/report', wrong, headers)[0] == 409
            and request('/api/v1/nodes')[1]['nodes'][0]['metrics'] == before,
            'missing inventory cache rejects atomically and requests a full report without clearing data')
    wrong['reuse_inventory'] = '../invalid'
    require(request('/api/v1/agent/report', wrong, headers)[0] == 400,
            'inventory references accept only bounded content digests')
    wrong = copy.deepcopy(compact)
    wrong['metrics']['users'] = report['metrics']['users']
    require(request('/api/v1/agent/report', wrong, headers)[0] == 400,
            'compact reports cannot mix cached and contradictory inventory')
    wrong = copy.deepcopy(compact)
    wrong['protocol_version'] = 1
    require(request('/api/v1/agent/report', wrong, headers)[0] == 400,
            'old wire protocol cannot silently publish an incomplete snapshot')
    status, restored = request('/api/v1/agent/report', report, headers)
    require(status == 200 and restored['inventory_cached'] == key
            and request('/api/v1/nodes')[1]['nodes'][0]['metrics'] == before,
            'full retransmission restores the same cache and remains retry-idempotent')
