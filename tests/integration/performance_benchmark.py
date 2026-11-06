"""Compare identical synthetic monitoring work in the disposable PostgreSQL/TLS fixture.
Never import this as a production load generator; the outer runner verifies isolation.
"""
import copy
import http.client
import json
import os
from pathlib import Path
import subprocess
import time
import uuid
from datetime import datetime, timedelta, timezone
from inventory_cache_smoke import FIELDS


def exercise(previous, current, root, environment, context, enrollment, log, require):
    def db(database, sql):
        command = ['/usr/lib/postgresql/18/bin/psql', '-X', '-q', '-t', '-A', '-h',
                   '127.0.0.1', '-p', '55432', '-U', 'smoke', '-d', database,
                   '-v', 'ON_ERROR_STOP=1']
        result = subprocess.run(command, input=sql, env=environment, capture_output=True,
                                text=True, timeout=15)
        assert result.returncode == 0, 'Private performance fixture database operation failed'
        return result.stdout.strip()

    def cpu(pid):
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(') ', 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf('SC_CLK_TCK')

    def rss(pid):
        return int(Path(f'/proc/{pid}/statm').read_text().split()[1]) * os.sysconf('SC_PAGE_SIZE')

    def start(binary, env):
        process = subprocess.Popen([str(binary.resolve())], cwd=root, env=env,
                                   stdout=log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            connection = http.client.HTTPSConnection('127.0.0.1', 58444, context=context, timeout=10)
            try:
                connection.request('GET', '/api/v1/health/ready')
                response = connection.getresponse(); response.read()
                if response.status == 200:
                    return process, connection
            except OSError:
                pass
            connection.close()
            time.sleep(.1)
        process.terminate(); process.wait(timeout=20)
        raise AssertionError('Private performance backend did not start')

    results = []
    for label, binary in [('before', previous), ('after_legacy_agent', current), ('after', current)]:
        database = 'performance_' + label
        db('postgres', 'CREATE DATABASE ' + database)
        env = environment | {'DATABASE_URL': f'postgresql://smoke@127.0.0.1:55432/{database}',
                             'EASYMANAGE_BIND': '127.0.0.1:58444',
                             'EASYMANAGE_DATABASE_MAX_CONNECTIONS': '4'}
        process, connection = start(binary, env)
        try:
            def request(path, payload=None, token=None, expected=200):
                body = None if payload is None else json.dumps(payload, separators=(',', ':')).encode()
                headers = {'Content-Type': 'application/json'}
                if token:
                    headers['x-easymanage-agent-token'] = token
                if path.endswith('/enroll'):
                    headers['x-easymanage-enrollment-token'] = enrollment
                connection.request('GET' if body is None else 'POST', path, body, headers)
                response = connection.getresponse(); raw = response.read()
                assert response.status == expected, 'Unexpected synthetic report response'
                return json.loads(raw) if raw else None, len(body or b'')

            node = str(uuid.uuid4())
            inventory = dict(hostname='performance-fixture', os_name='Linux', os_version='fixture',
                             kernel_version='fixture', architecture='x86_64', cpu_model='fixture',
                             cpu_logical_cores=36, cpu_physical_cores=18)
            identity, _ = request('/api/v1/agent/enroll', dict(protocol_version=1, requested_node_id=node,
                                  agent_version='fixture', inventory=inventory))
            token = identity['agent_token']
            users = [dict(username=f'fixture-user-{i}', uid=1000+i, gid=1000+i,
                          home=f'/home/fixture-user-{i}', shell='/bin/bash', groups=['users'],
                          sessions=0, session_sources=[], process_count=None, locked=None,
                          account_status='unknown') for i in range(200)]
            packages = [dict(name=f'fixture-package-{i}', version='1.0') for i in range(400)]
            now = datetime.now(timezone.utc)
            base_metrics = dict(cpu_usage_percent=1.0, load_average_one=0., load_average_five=0.,
                                load_average_fifteen=0., memory_total_bytes=1024, memory_used_bytes=100,
                                swap_total_bytes=0, swap_used_bytes=0, disks=[], networks=[], gpus=[],
                                users=users, software_packages=packages,
                                privilege=dict(mode='basic', username='fixture', uid=1000))
            cached = None
            sent = 0
            # Same cadence ratio as production: six 10-second reports per inventory refresh.
            count = 240
            start_lsn = db(database, 'SELECT pg_current_wal_insert_lsn()')
            started, cpu_before, memory_peak = time.monotonic(), cpu(process.pid), rss(process.pid)
            for i in range(count):
                full = (i % 6 == 0)
                metrics = copy.deepcopy(base_metrics)
                metrics.update(collected_at=(now-timedelta(seconds=count-i)).isoformat(),
                               inventory_collected_at=(now-timedelta(seconds=count-(i//6)*6)).isoformat(),
                               uptime_seconds=i)
                report = dict(protocol_version=1, node_id=node, agent_version='fixture',
                              inventory=inventory, metrics=metrics)
                if not full:
                    report.update(protocol_version=2, reuse_software_inventory=True)
                    metrics.pop('software_packages')
                    if label == 'after':
                        assert cached, 'New backend did not negotiate inventory caching'
                        report.update(protocol_version=3, reuse_inventory=cached)
                        for field in FIELDS:
                            metrics.pop(field, None)
                response, size = request('/api/v1/agent/report', report, token)
                cached = response.get('inventory_cached'); sent += size
                memory_peak = max(memory_peak, rss(process.pid))
            elapsed, cpu_seconds = time.monotonic()-started, cpu(process.pid)-cpu_before
            wal_bytes = int(db(database, f"SELECT pg_wal_lsn_diff(pg_current_wal_insert_lsn(),'{start_lsn}')"))
            listing, _ = request('/api/v1/nodes')
            observed = next(n for n in listing['nodes'] if n['id'] == node)['metrics']
            require(len(observed['users']) == 200 and observed['uptime_seconds'] == count-1
                    and observed['software_packages'] == packages
                    and int(db(database, 'SELECT COUNT(*) FROM metrics_samples')) == count,
                    f'{label} benchmark retains every historical sample and complete latest inventory')
            results.append(dict(version=label, reports=count, request_body_bytes=sent,
                                wal_generated_bytes=wal_bytes, server_cpu_seconds=round(cpu_seconds, 4),
                                elapsed_seconds=round(elapsed, 4), peak_observed_rss_bytes=memory_peak))
            if label == 'before':
                connection.close(); process.terminate(); process.wait(timeout=20)
                process, connection = start(current, env)
                upgraded = next(n for n in request('/api/v1/nodes')[0]['nodes'] if n['id'] == node)['metrics']
                require(upgraded == observed and db(database, 'SELECT MAX(version) FROM _sqlx_migrations') == '22'
                        and db(database, "SELECT metrics ? 'users' FROM metrics_latest") == 'f',
                        'real schema-21 upgrade separates existing inventory while preserving every API value')
                connection.close(); process.terminate(); process.wait(timeout=20)
                db(database, (Path(__file__).resolve().parents[2]/'deploy/database/rollback-inventory-cache.sql').read_text())
                process, connection = start(previous, env)
                rolled_back = next(n for n in request('/api/v1/nodes')[0]['nodes'] if n['id'] == node)['metrics']
                require(rolled_back == observed and int(db(database, 'SELECT COUNT(*) FROM metrics_samples')) == count,
                        'inventory-cache rollback restores the old backend without losing history or snapshot fields')
            if label == 'after':
                other = str(uuid.uuid4())
                second, _ = request('/api/v1/agent/enroll', dict(protocol_version=1, requested_node_id=other,
                                    agent_version='fixture', inventory=inventory))
                forged = copy.deepcopy(report); forged['node_id'] = other
                request('/api/v1/agent/report', forged, second['agent_token'], expected=409)
                require(db(database, f"SELECT COUNT(*) FROM metrics_latest WHERE node_id='{other}'") == '0',
                        'one node cannot reuse another node inventory even with its own valid Agent token')
                db(database, f"UPDATE node_snapshot_inventory SET digest=NULL WHERE node_id='{node}'")
                request('/api/v1/agent/report', report, token, expected=409)
                full_report = copy.deepcopy(report)
                full_report.pop('reuse_inventory', None); full_report.pop('reuse_software_inventory', None)
                full_report['protocol_version'] = 1
                full_report['metrics'].update(copy.deepcopy(base_metrics))
                request('/api/v1/agent/report', full_report, token)
                require(next(n for n in request('/api/v1/nodes')[0]['nodes'] if n['id'] == node)['metrics']['users'] == observed['users'],
                        'cache reset requests a full retransmission with the existing identity and restores all inventory')

        finally:
            if connection:
                connection.close()
            process.terminate(); process.wait(timeout=20)
            db('postgres', 'DROP DATABASE ' + database)
    before, after = results[0], results[-1]
    require(after['request_body_bytes'] < before['request_body_bytes'] * .65,
            'identical-cadence inventory caching reduces report payload by more than 35 percent')
    print('PERFORMANCE_BENCHMARK ' + json.dumps(results), flush=True)
