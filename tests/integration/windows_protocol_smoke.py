"""Synthetic Windows wire data, called only from the guarded private HTTP fixture."""
import copy
import uuid
from datetime import datetime, timezone
from inventory_cache_smoke import FIELDS

def exercise(request, require, enrollment, enrollment_token, report):
    node_id=str(uuid.uuid4())
    inventory=copy.deepcopy(report['inventory'])
    inventory.update(hostname='windows-fixture',os_name='Windows',os_version='Windows 11 fixture',kernel_version='10.0.fixture',architecture='x86_64')
    status,identity=request('/api/v1/agent/enroll',enrollment|{'requested_node_id':node_id,'inventory':inventory},{'x-easymanage-enrollment-token':enrollment_token})
    require(status==200,'Windows enrollment preserves the assigned node identity')
    metrics=copy.deepcopy(report['metrics'])
    metrics.update(collected_at=datetime.now(timezone.utc).isoformat(),load_average_one=None,load_average_five=None,load_average_fifteen=None,
        privilege={'mode':'full','username':'NT AUTHORITY\\SYSTEM','uid':4294967295,'native':{'platform':'windows','principal':'S-1-5-18','administrator':True,'capabilities':['metrics','terminal','service']}},
        users=[{'username':'Fixture','uid':4294967295,'gid':4294967295,'native_id':'S-1-5-21-1000','administrator':False,'home':'C:/Users/Fixture','shell':'pwsh','groups':[],'sessions':None,'process_count':None,'locked':False}],
        disks=[{'name':'C:','mount_point':'C:/','file_system':'NTFS','total_bytes':1000000,'available_bytes':500000,'read_bytes_per_second':None,'written_bytes_per_second':None}],agent_control={'revision':0,'paused':False})
    windows=report|{'node_id':node_id,'inventory':inventory,'metrics':metrics,'protocol_version':1}
    headers={'x-easymanage-agent-token':identity['agent_token']}
    status,accepted=request('/api/v1/agent/report',windows,headers)
    require(status==200,'native Windows metrics with unknown POSIX load averages are accepted')
    node=next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==node_id)
    require(node['os_name']=='Windows' and node['metrics']['privilege']==metrics['privilege'] and node['metrics']['load_average_one'] is None and node['metrics']['users'][0]['native_id']=='S-1-5-21-1000' and node['metrics']['disks'][0]['mount_point']=='C:/',
        'Windows SID, Administrator token, drive paths and unavailable values survive HTTPS/PostgreSQL round trip')
    compact=copy.deepcopy(windows)
    compact.update(protocol_version=3,reuse_inventory=accepted['inventory_cached'],reuse_software_inventory=True)
    compact['metrics'].pop('software_packages',None)
    for field in FIELDS:compact['metrics'].pop(field,None)
    require(request('/api/v1/agent/report',compact,headers)[0]==200,'Windows reports use the same acknowledged inventory-cache protocol')
