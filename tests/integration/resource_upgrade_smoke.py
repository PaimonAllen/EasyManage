"""Schema 16 -> current -> 16 -> current, using the private test database."""
import json,subprocess,time,urllib.request,urllib.error,uuid
from pathlib import Path

def exercise(previous,current,root,env,url,context,enrollment,log,require):
    opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=context))
    def request(path,body=None,headers=None):
        with opener.open(urllib.request.Request(url+path,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json'}|(headers or {})),timeout=5) as response:
            return json.load(response)
    def database(sql):
        command=['/usr/lib/postgresql/18/bin/psql','-X','-A','-t','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c',sql]
        result=subprocess.run(command,env=env,capture_output=True,text=True,timeout=10)
        assert result.returncode==0,'Private migration database operation failed'
        return result.stdout.strip()
    def stop(process):
        if process and process.poll() is None:process.terminate();process.wait(timeout=20)
    def start(binary):
        process=subprocess.Popen([str(binary.resolve())],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            try:
                request('/api/v1/health/ready');return process
            except (OSError,urllib.error.URLError):time.sleep(.1)
        stop(process);raise AssertionError('Private migration fixture did not become ready')
    process=None;node=str(uuid.uuid4());packages=[{'name':'retained-upgrade-fixture','version':'1'}]
    try:
        process=start(previous);assert database('SELECT MAX(version) FROM _sqlx_migrations')=='16'
        inventory={'hostname':'upgrade-fixture','os_name':'Linux','os_version':'fixture','kernel_version':'fixture','architecture':'x86_64','cpu_model':'fixture','cpu_logical_cores':1,'cpu_physical_cores':1}
        identity=request('/api/v1/agent/enroll',{'protocol_version':1,'agent_version':'fixture','requested_node_id':node,'inventory':inventory},{'x-easymanage-enrollment-token':enrollment})
        from datetime import datetime,timezone
        metrics={'collected_at':datetime.now(timezone.utc).isoformat(),'cpu_usage_percent':2.5,'memory_total_bytes':1024,'memory_used_bytes':128,'swap_total_bytes':0,'swap_used_bytes':0,'uptime_seconds':1,
            'load_average_one':0,'load_average_five':0,'load_average_fifteen':0,'disks':[],'networks':[],'gpus':[],'software_packages':packages}
        request('/api/v1/agent/report',{'protocol_version':1,'node_id':node,'agent_version':'fixture','inventory':inventory,'metrics':metrics},{'x-easymanage-agent-token':identity['agent_token']})
        stop(process);process=start(current)
        retained_document='# retained Markdown across rollback\n'
        database("INSERT INTO platform_documents(slug,content,revision) VALUES('gpu-comparison','# retained Markdown across rollback'||chr(10),1)")
        external_id=str(uuid.uuid4())
        database(f"INSERT INTO external_platforms(id,name,url,revision,deleted_at) VALUES('{external_id}','retained external','https://compute.example/retained',2,NOW())")
        database(f"INSERT INTO agent_service_controls(node_id,desired_stopped,state,revision) VALUES('{node}',TRUE,'stopped',1)")
        observed=next(n for n in request('/api/v1/nodes')['nodes'] if n['id']==node)
        require(observed['metrics']['software_packages']==packages and observed['metrics']['cpu_usage_percent']==2.5
            and database(f"SELECT COUNT(*) FROM metrics_samples WHERE node_id='{node}'")=="1"
            and database(f"SELECT metrics ? 'software_packages' FROM metrics_latest WHERE node_id='{node}'")=='f',
            'schema upgrade preserves legacy software and history while separating dynamic snapshots')
        stop(process);process=None
        database((Path(__file__).resolve().parents[2]/'deploy/database/rollback-inventory-cache.sql').read_text())
        database((Path(__file__).resolve().parents[2]/'deploy/database/rollback-agent-service.sql').read_text())
        database((Path(__file__).resolve().parents[2]/'deploy/database/rollback-external-platforms.sql').read_text())
        database((Path(__file__).resolve().parents[2]/'deploy/database/rollback-platform-documents.sql').read_text())
        database((Path(__file__).resolve().parents[2]/'deploy/database/rollback-resource-accounting.sql').read_text())
        process=start(previous)
        require(request('/api/v1/nodes')['nodes'][0]['metrics']['software_packages']==packages
            and database('SELECT MAX(version) FROM _sqlx_migrations')=='16',
            'emergency rollback restores old backend compatibility and retains software and monitoring history')
        compact={'protocol_version':2,'reuse_software_inventory':True,'node_id':node,'agent_version':'fixture','inventory':inventory,'metrics':metrics|{'software_packages':[]}}
        try:request('/api/v1/agent/report',compact,{'x-easymanage-agent-token':identity['agent_token']})
        except urllib.error.HTTPError as error:require(error.code==400,'old backend rejects negotiated compact reports instead of clearing inventory')
        else:raise AssertionError('Old backend silently accepted an incompatible compact report')
        stop(process);process=start(current)
        require(database('SELECT MAX(version) FROM _sqlx_migrations')=='22','forward migration remains repeatable after an emergency rollback')
        require(request('/api/v1/documents/gpu-comparison')['content']==retained_document,'document rollback and upgrade retain saved Markdown instead of restoring the initial reference')
        require(any(item['id']==external_id and item['deleted_at'] is not None for item in request('/api/v1/external-platforms')['platforms']),'external platform rollback and upgrade retain shortcut identity and soft-delete state')
        require(request('/api/v1/nodes')['nodes'][0]['agent_service_state']=='stopped' and not request('/api/v1/nodes')['nodes'][0]['online'],
            'service stop remains distinct from monitoring pause through compatibility rollback and upgrade')
        stop(process);process=None
        database(f"DELETE FROM metrics_samples WHERE node_id='{node}'; DELETE FROM agent_service_controls WHERE node_id='{node}'; DELETE FROM nodes WHERE id='{node}'; DELETE FROM platform_documents WHERE slug='gpu-comparison'; DELETE FROM external_platforms WHERE id='{external_id}'")
    finally:stop(process)
