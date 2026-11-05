#!/usr/bin/env python3
"""Server integration smoke test, deliberately refusing the host namespaces.

Run ONLY in a disposable VM or a bwrap namespace with private network, private
tmpfs /tmp, read-only host mounts and a non-root uid. Required flags identify
the caller's ORIGINAL namespace ids so accidental host execution is rejected.
The runner creates PostgreSQL, TLS, fixtures and logs only in its new /tmp tree.
No existing database, service manager, Agent, host SSH or Nginx is contacted.
Optionally add --agent-binary to exercise the actual collector in the private
namespaces, including heartbeat, persistent identity, restart and shutdown.

Example outer launch (adapt read-only /tmp/work binding to the repository):
  bwrap --unshare-all --die-with-parent --new-session --ro-bind / / \\
    --tmpfs /tmp --proc /proc --dev /dev --dir /tmp/work \\
    --ro-bind "$PWD" /tmp/work --chdir /tmp/work \\
    /usr/bin/python3 /tmp/work/tests/integration/isolated_smoke.py \\
    --server-binary /tmp/work/target/release/easymanage-server \\
    --host-netns "$(readlink /proc/self/ns/net)" \\
    --host-mntns "$(readlink /proc/self/ns/mnt)"

This is an isolated namespace test, not evidence of tests inside a full VM.
"""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import shutil
import socket
import ssl
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid


def ensure_sandbox(args):
    for kind, original in (("net", args.host_netns), ("mnt", args.host_mntns)):
        current = os.readlink(f"/proc/self/ns/{kind}")
        if not re.fullmatch(rf"{kind}:\[\d+\]", original) or current == original:
            raise RuntimeError(f"Refusing tests in the original {kind} namespace")
    mounts = Path("/proc/self/mountinfo").read_text().splitlines()
    if not any(line.split()[4] == "/tmp" and line.split(" - ", 1)[1].split()[0] == "tmpfs" for line in mounts):
        raise RuntimeError("Refusing tests without a private tmpfs mounted at /tmp")
    interfaces = {name for _, name in socket.if_nameindex()}
    if interfaces != {"lo"}:
        raise RuntimeError("Refusing tests with a non-loopback network interface")
    if os.geteuid() == 0:
        raise RuntimeError("Use a non-root uid inside the disposable environment (PostgreSQL requirement)")
    if not args.server_binary.is_file():
        raise RuntimeError("Build the server before entering the isolated environment")
    if args.agent_binary is not None and not args.agent_binary.is_file():
        raise RuntimeError("Build the optional Agent before entering the isolated environment")


def run_checked(*command, env, log):
    subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=30)


def stop_process(process):
    if process is None or process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def require(condition, description):
    if not condition:
        raise AssertionError(description)
    print(f"PASS {description}", flush=True)


def parse_timestamp(value):
    """Parse Chrono RFC3339 at Python's microsecond precision, including 3.10."""
    match = re.fullmatch(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})", value)
    if match is None:
        raise ValueError("Timestamp is not a timezone-aware RFC3339 value")
    prefix, fractional, offset = match.groups()
    fraction = "." + fractional[:6].ljust(6, "0") if fractional else ""
    return datetime.fromisoformat(prefix + fraction + ("+00:00" if offset == "Z" else offset))


def test_parse_timestamp_nanoseconds():
    """Pure regression test; the smoke runner calls this inside isolation."""
    expected = datetime(2026, 9, 21, 9, 34, 25, 229600, tzinfo=timezone.utc)
    for value in ("2026-09-21T09:34:25.229600957+00:00", "2026-09-21T09:34:25.229600957Z",
                  "2026-09-21T17:34:25.229600957+08:00", "2026-09-21T09:34:25.229600Z"):
        assert parse_timestamp(value) == expected
    assert parse_timestamp("2026-09-21T09:34:25.2Z") == expected.replace(microsecond=200000)
    assert parse_timestamp("2026-09-21T09:34:25.000000999Z") == expected.replace(microsecond=0)
    assert parse_timestamp("2026-09-21T09:34:25Z") == expected.replace(microsecond=0)
    for invalid in ("2026-09-21T09:34:25", "not-a-timestamp", "2026-09-21T09:34:25.1234567890Z"):
        try:
            parse_timestamp(invalid)
        except ValueError:
            continue
        raise AssertionError("RFC3339 parser accepted an invalid or timezone-free value")


def create_test_certificates(root, env, log):
    """Use a CA-signed non-CA leaf; Rustls correctly rejects CA leaves."""
    ca, ca_key = root / "ca.crt", root / "ca.key"
    certificate, key, csr = root / "server.crt", root / "server.key", root / "server.csr"
    run_checked("/usr/bin/openssl", "req", "-x509", "-nodes", "-newkey", "rsa:2048", "-days", "1", "-sha256",
                "-subj", "/CN=EasyManage Isolated Smoke CA", "-addext", "basicConstraints=critical,CA:TRUE,pathlen:0",
                "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-addext", "subjectKeyIdentifier=hash",
                "-keyout", str(ca_key), "-out", str(ca), env=env, log=log)
    run_checked("/usr/bin/openssl", "req", "-new", "-nodes", "-newkey", "rsa:2048", "-sha256", "-subj", "/CN=127.0.0.1",
                "-keyout", str(key), "-out", str(csr), env=env, log=log)
    extensions = root / "server.extensions"
    extensions.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\n"
                          "extendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\nsubjectKeyIdentifier=hash\n"
                          "authorityKeyIdentifier=keyid,issuer\n")
    run_checked("/usr/bin/openssl", "x509", "-req", "-in", str(csr), "-CA", str(ca), "-CAkey", str(ca_key), "-CAcreateserial",
                "-out", str(certificate), "-days", "1", "-sha256", "-extfile", str(extensions), env=env, log=log)
    run_checked("/usr/bin/openssl", "verify", "-CAfile", str(ca), "-purpose", "sslserver", "-verify_ip", "127.0.0.1",
                str(certificate), env=env, log=log)
    ca_key.chmod(0o600)
    ca.chmod(0o600)
    key.chmod(0o600)
    return ca, certificate, key


def print_isolated_log_tail(log, label, secrets_to_redact):
    """Only accepts the fresh test's own log handle, never reads host logs."""
    log.flush()
    length = os.fstat(log.fileno()).st_size
    tail = os.pread(log.fileno(), 8192, max(0, length - 8192)).decode("utf-8", errors="replace")
    for secret in secrets_to_redact:
        if secret:
            tail = tail.replace(secret, "[redacted]")
    tail = re.sub(r"\b[0-9a-fA-F]{64}\b", "[redacted-token-or-digest]", tail)
    tail = re.sub(r"(postgres(?:ql)?://)[^@\s]+@", r"\1[redacted]@", tail)
    tail = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", tail)
    lines = tail.splitlines()[-20:]
    print(f"DIAGNOSTIC {label}: last {len(lines)} isolated log lines", flush=True)
    for line in lines:
        print(line[:512], flush=True)


def exercise_api(base_url, context, enrollment_token, node_id):
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))

    def request(path, payload=None, headers=None):
        body = None if payload is None else json.dumps(payload).encode()
        request_headers = dict(headers or {})
        if body is not None:
            request_headers["Content-Type"] = "application/json"
        query = urllib.request.Request(base_url + path, data=body, headers=request_headers)
        try:
            response = opener.open(query, timeout=12)
        except urllib.error.HTTPError as error:
            response = error
        with response:
            raw = response.read(2 * 1024 * 1024)
            result = json.loads(raw) if response.headers.get_content_type() == "application/json" else raw.decode("utf-8", errors="replace")
            return response.status, result

    require(request("/api/v1/health/ready")[0] == 200, "temporary database migrations and readiness")
    status, dashboard = request("/")
    require(status == 200 and "isolated-smoke-dashboard" in dashboard, "dashboard static content without login token")
    status, nodes = request("/api/v1/nodes")
    require(status == 200 and nodes["nodes"] == [], "node listing without dashboard authentication")
    network_before=request('/api/v1/platform-network')[1]
    assert network_before['agents']==[] and network_before['agents_available']
    browser_before=next(g for g in network_before['server']['groups'] if g['group']=='http_browser')
    sentinel={'host':'invalid-address','port':22,'fixture':'NETWORK_CONTENT_MUST_NOT_BE_RETAINED'}
    require(request('/api/v1/machines/probe',sentinel,{'Origin':base_url})[0]==422,'network fixture request rejected without acting on a target')
    network_after=request('/api/v1/platform-network')[1]
    browser_after=next(g for g in network_after['server']['groups'] if g['group']=='http_browser')
    require(browser_after['received_bytes']-browser_before['received_bytes']==len(json.dumps(sentinel).encode())
            and browser_after['started']>=browser_before['started']+2 and browser_after['failed']>browser_before['failed']
            and browser_after['submitted_bytes']>browser_before['submitted_bytes'],
            'HTTP middleware streams and counts exact consumed request bytes, response bytes and failures')
    require('NETWORK_CONTENT_MUST_NOT_BE_RETAINED' not in json.dumps(network_after)
            and all(set(g)=={'group','started','completed','failed','in_flight','open_connections','received_bytes','submitted_bytes'} for g in network_after['server']['groups']),
            'network endpoint contains only fixed categories and counters, never payloads or credentials')
    inventory = {
        "hostname": "isolated-smoke-node", "os_name": "Smoke Linux", "os_version": "1",
        "kernel_version": "synthetic", "architecture": "x86_64", "cpu_model": "Synthetic CPU",
        "cpu_logical_cores": 2, "cpu_physical_cores": 1,
    }
    enrollment = {"protocol_version": 1, "requested_node_id": node_id, "agent_version": "0.1.0", "inventory": inventory}
    require(request("/api/v1/agent/enroll", enrollment)[0] == 401, "enrollment without a secret is rejected")
    require(request("/api/v1/agent/enroll", enrollment, {"x-easymanage-enrollment-token": "invalid"})[0] == 401,
            "wrong enrollment secret is rejected")
    require(request("/api/v1/machines/probe", {"host":"10.0.0.8","port":22}, {"Origin":"https://foreign.invalid"})[0]==403,
            "cross-origin machine probe is rejected before SSH")
    require(request(f"/api/v1/machines/{node_id}/agent/stop",{'confirmation':node_id,'expected_revision':0}, {'Origin':'https://foreign.invalid'})[0]==403,
            'cross-origin complete Agent shutdown is rejected before credentials, SSH or state writes')
    add_fixture={"host":"10.0.0.8","port":22,"display_name":"fixture","server_url":base_url,
                 "fingerprint":"fixture","username":"tester","password":"fixture"}
    require(request("/api/v1/machines",add_fixture,{"Origin":"https://foreign.invalid"})[0]==403,
            "cross-origin machine deployment is rejected before credential use")
    require(request(f"/api/v1/machines/{node_id}/redeploy",{"password":"fixture-secret-marker"},{"Origin":"https://foreign.invalid"})[0]==403,
            "cross-origin redeployment is rejected before credentials, SSH or database mutation")
    for invalid in [add_fixture|{"auth_method":"fixture-secret-marker"},
                    add_fixture|{"auth_method":"private_key","private_key":"fixture-secret-marker"},
                    add_fixture|{"password":["fixture-secret-marker"]}]:
        status, rejected = request("/api/v1/machines",invalid,{"Origin":base_url})
        require(status==400 and "fixture-secret-marker" not in json.dumps(rejected),
                "invalid authentication requests never echo secret values")
    require(request("/api/v1/machines/probe",{"host":"127.0.0.1","port":22},{"Origin":base_url})[0]==400,
            "onboarding probe refuses loopback SSH and metadata-service addresses")
    status, identity = request("/api/v1/agent/enroll", enrollment, {"x-easymanage-enrollment-token": enrollment_token})
    require(status == 200 and identity["node_id"] == node_id, "authenticated enrollment preserves requested identity")
    agent_token = identity["agent_token"]
    collected = datetime.now(timezone.utc)
    metrics = {
        "collected_at": collected.isoformat(), "cpu_usage_percent": 37.5,
        "software_packages": [{"name":f"fixture-package-{i}","version":"1.0.0"} for i in range(400)],
        "host_security": {"collected_at": (collected-timedelta(minutes=30)).isoformat(), "interval_seconds": 3600,
                          "apparmor_enabled": True, "aslr_mode": 2, "ptrace_scope": 1, "kernel_lockdown": "integrity"},
        "cpu_frequency": {"collected_at": (collected-timedelta(seconds=20)).isoformat(),"average_mhz":2800,"minimum_mhz":800,"maximum_mhz":4200,"logical_cpus":4},
        "cpu_sockets": [{"id":2,"model":"Fixture package 2","logical_cpus":2,"physical_cores":1,"usage_percent":20,"frequency":None,"temperature_celsius":None},
                        {"id":7,"model":"Fixture package 7","logical_cpus":2,"physical_cores":1,"usage_percent":70,"frequency":None,"temperature_celsius":51}],
        "temperatures": {"collected_at": (collected - timedelta(seconds=20)).isoformat(),
                         "cpu_celsius": 51.5, "ssds": [{"device": "nvme0", "temperature_celsius": 42.0}]},
        "load_average_one": 0.4, "load_average_five": 0.3, "load_average_fifteen": 0.2,
        "memory_total_bytes": 1073741824, "memory_used_bytes": 536870912,
        "swap_total_bytes": 0, "swap_used_bytes": 0, "uptime_seconds": 120,
        "disks": [
            {"name": "synthetic-disk", "mount_point": "/", "file_system": "ext4", "total_bytes": 1000000, "available_bytes": 600000, "read_bytes_per_second": 1536.0, "written_bytes_per_second": 768.0},
            {"name": "synthetic-disk", "mount_point": "/data", "file_system": "ext4", "total_bytes": 1000000, "available_bytes": 600000, "read_bytes_per_second": 1536.0, "written_bytes_per_second": 768.0},
            {"name":"198.51.100.1:/export","mount_point":"/nfs","file_system":"nfs4","total_bytes":0,"available_bytes":0,"read_bytes_per_second":None,"written_bytes_per_second":None,
             "mount":{"device_id":"0:41","root":"/","source":"198.51.100.1:/export","network":True,"read_only":False,"capacity_known":False,
                      "capacity_probe":{"interval_seconds":3600,"attempted_at":collected.isoformat(),"collected_at":None,"status":"timeout"}}},
        ],
        "block_devices": [{"name": "synthetic-disk", "path": "/dev/synthetic-disk", "kind": "disk", "size_bytes": 1000000,
                           "filesystem": None, "label": None, "mount_points": [], "model": "Test Drive", "transport": "virtio",
                           "rotational": False, "removable": False, "read_only": False, "partition_table": "gpt",
                           "partition_label": None, "start_bytes": None,
                           "children": [{"name": "synthetic-disk1", "path": "/dev/synthetic-disk1", "kind": "part", "size_bytes": 500000,
                                         "filesystem": "ext4", "label": None, "mount_points": [], "model": None, "transport": None,
                                         "rotational": None, "removable": False, "read_only": False, "partition_table": None,
                                         "partition_label": "data", "start_bytes": 4096, "children": []}]}],
        "disk_collected_at": collected.isoformat(),
        "networks": [{"interface": "synthetic0", "received_bytes_per_second": 128.0, "transmitted_bytes_per_second": 64.0, "total_received_bytes": 1024, "total_transmitted_bytes": 512}],
        "gpus": [{"index": 0, "name": "Synthetic GPU", "utilization_percent": 12.0, "memory_total_bytes": 2048, "memory_used_bytes": 512, "temperature_celsius": 30.0, "power_watts": 20.0}],
        "platform_process": {"pid": 321, "cpu_percent_of_host": 0.25, "resident_memory_bytes": 1048576,
                             "disk_read_bytes_per_second": 128.0, "disk_written_bytes_per_second": 64.0},
        "platform_storage": {"collected_at": collected.isoformat(), "binary_bytes": 1000,
                             "state_bytes": 200, "web_bytes": None, "database_bytes": None,
                             "total_bytes": 1200},
        "privilege": {"mode": "basic", "username": "smoke", "uid": 1000},
        "users": [{"username": "root", "uid": 0, "gid": 0, "home": "/root", "shell": "/bin/bash", "groups": ["root"], "sessions": 1, "session_sources": ["logind", "ssh"], "session_evidence": {"terminal_records": 0, "logind_sessions": 1, "ssh_processes": 1}, "process_count": None, "locked": None, "account_status": "normal"}],
        "interfaces": [], "overlay_networks": [{"provider": "tailscale", "state": "healthy", "detail": "fixture", "networks": []}],
        "routes": [], "proxy_services": [], "collection_issues": [],
        "inventory_collected_at": collected.isoformat(),
    }
    report = {"protocol_version": 1, "node_id": node_id, "agent_version": "0.1.0", "inventory": inventory, "metrics": metrics}
    require(request("/api/v1/agent/report", report)[0] == 401, "reporting without an Agent token is rejected")
    require(request("/api/v1/agent/report", report, {"x-easymanage-agent-token": "invalid"})[0] == 401,
            "reporting with a wrong Agent token is rejected")
    require(request("/api/v1/agent/report", report, {"x-easymanage-agent-token": agent_token})[0] == 200,
            "authenticated metrics reporting")
    compact=json.loads(json.dumps(report));compact['reuse_software_inventory']=True;compact['protocol_version']=2
    compact['metrics'].pop('software_packages')
    status,accepted=request('/api/v1/agent/report',compact,{'x-easymanage-agent-token':agent_token})
    require(status==200 and accepted['software_inventory_cached'] is True
            and len(json.dumps(compact))<len(json.dumps(report))*.6,
            'negotiated inventory reuse removes repeated package payloads and accepts retry without duplicate history')
    with opener.open(base_url+'/api/v1/nodes?include_software=false',timeout=5) as response:
        etag=response.headers['ETag'];light=json.load(response)
    require(not light['nodes'][0]['metrics']['software_packages'],
            'overview reads omit software packages without altering the full inventory API')
    require(request('/api/v1/nodes?include_software=false')[1]==light,
            'adjacent public fleet reads reuse the same serialized snapshot without mixing full/light views')
    try:
        opener.open(urllib.request.Request(base_url+'/api/v1/nodes?include_software=false',headers={'If-None-Match':etag}),timeout=5)
    except urllib.error.HTTPError as error:
        require(error.code==304 and error.read()==b'', 'unchanged fleet view returns a body-free conditional response')
    else:raise AssertionError('Unchanged overview response should be 304')
    require(request('/api/v1/nodes')[1]['nodes'][0]['metrics']['software_packages']==metrics['software_packages'],
            'full software list survives compact heartbeats and remains available to existing clients')
    invalid_temperature = json.loads(json.dumps(report))
    invalid_temperature["metrics"]["temperatures"]["cpu_celsius"] = 999
    require(request("/api/v1/agent/report", invalid_temperature, {"x-easymanage-agent-token": agent_token})[0] == 400,
            "invalid sensor temperature rejected before database writes")
    invalid_frequency=json.loads(json.dumps(report))
    invalid_frequency['metrics']['cpu_frequency']['maximum_mhz']=1000
    require(request('/api/v1/agent/report',invalid_frequency,{'x-easymanage-agent-token':agent_token})[0]==400,
            'inconsistent CPU frequency range rejected before database writes')
    invalid_sockets=json.loads(json.dumps(report));invalid_sockets['metrics']['cpu_sockets'][1]['id']=2
    require(request('/api/v1/agent/report',invalid_sockets,{'x-easymanage-agent-token':agent_token})[0]==400,
            'duplicate CPU package identities cannot inflate the detected socket count')
    invalid_storage = json.loads(json.dumps(report))
    invalid_storage["metrics"]["platform_storage"]["total_bytes"] = 5000
    require(request("/api/v1/agent/report", invalid_storage, {"x-easymanage-agent-token": agent_token})[0] == 400,
            "inconsistent Agent storage footprint rejected before database writes")
    invalid_hardware = json.loads(json.dumps(report))
    invalid_hardware["metrics"]["hardware_details"] = {
        "collected_at": collected.isoformat(), "processors": ["fixture"] * 129,
        "motherboard": [], "memory": [], "graphics": [], "displays": [],
        "disks": [], "audio": [], "network": [], "notes": [],
    }
    require(request("/api/v1/agent/report", invalid_hardware, {"x-easymanage-agent-token": agent_token})[0] == 400,
            "oversized hardware inventory rejected before database writes")
    live = {"protocol_version": 1, "node_id": node_id, "metrics": {
        "collected_at": collected.isoformat(), "cpu_usage_percent": 41.0,
        "memory_total_bytes": 1073741824, "memory_used_bytes": 600000000,
        "network_received_bytes_per_second": 150.0,
        "network_transmitted_bytes_per_second": 75.0,
    }}
    require(request("/api/v1/agent/live", live)[0] == 401,
            "live metrics require Agent authentication")
    require(request("/api/v1/agent/live", live, {"x-easymanage-agent-token": "invalid"})[0] == 401,
            "wrong Agent credential cannot publish live metrics")
    require(request("/api/v1/agent/live", live, {"x-easymanage-agent-token": agent_token})[0] == 204,
            "authenticated lightweight live metrics accepted")
    status, current = request("/api/v1/nodes/live")
    require(status == 200 and len(current["nodes"]) == 1
            and current["nodes"][0]["node_id"] == node_id
            and current["nodes"][0]["metrics"]["cpu_usage_percent"] == 41.0,
            "dashboard reads lightweight live metrics without full inventory")
    status, listing = request("/api/v1/nodes")
    node = next(item for item in listing["nodes"] if item["id"] == node_id)
    require(status == 200 and node["online"] and node["metrics"]["cpu_usage_percent"] == 37.5
            and node["metrics"]["users"][0]["username"] == "root" and len(node["metrics"]["gpus"]) == 1
            and node["metrics"]["block_devices"][0]["children"][0]["mount_points"] == []
            and node["metrics"]["users"][0]["session_sources"] == ["logind", "ssh"]
            and node["metrics"]["users"][0]["session_evidence"]["logind_sessions"] == 1
            and node["metrics"]["users"][0]["account_status"] == "normal"
            and node["metrics"]["overlay_networks"][0]["provider"] == "tailscale"
            and parse_timestamp(node["metrics"]["disk_collected_at"]) == collected,
            "CPU/memory/disk/network/GPU/user payload round trip")
    require(node['metrics']['host_security']['interval_seconds']==3600
            and parse_timestamp(node['metrics']['host_security']['collected_at'])==collected-timedelta(minutes=30)
            and node['metrics']['host_security']['apparmor_enabled'] is True,
            'hourly security metadata and original sample timestamp survive HTTPS/database round trip')
    network_mount=next(disk for disk in node['metrics']['disks'] if disk['mount_point']=='/nfs')
    require(network_mount['mount']['capacity_probe']['interval_seconds']==3600
            and network_mount['mount']['capacity_probe']['status']=='timeout'
            and network_mount['mount']['capacity_probe']['collected_at'] is None,
            'hourly network capacity timestamps and failure state survive HTTPS/database round trip')
    require(network_mount['mount']['network'] and network_mount['mount']['capacity_known'] is False
            and network_mount['mount']['source']=='198.51.100.1:/export',
            'network mount identity and explicit unknown capacity survive HTTPS/database round trip alongside legacy disks')
    status, history = request(f"/api/v1/nodes/{node_id}/history")
    require(status == 200 and history["node_id"] == node_id and history["window_seconds"] == 600
            and len(history["points"]) == 1 and history["points"][0]["cpu_usage_percent"] == 37.5
            and history["points"][0]["network_received_bytes_per_second"] == 128.0
            and history["points"][0]["disk_read_bytes_per_second"] == 1536.0
            and history["points"][0]["disk_written_bytes_per_second"] == 768.0
            and parse_timestamp(history["points"][0]["disk_collected_at"]) == collected
            and history["points"][0]["gpus"][0]["index"] == 0,
            "bounded history API returns CPU, memory, GPU, network and deduplicated disk I/O")
    require(len(history["points"]) == 1,
            "one-second live reports do not expand PostgreSQL history")
    require(node["metrics"]["temperatures"]["cpu_celsius"] == 51.5
            and history["points"][0]["temperatures"]["ssds"][0]["temperature_celsius"] == 42.0
            and parse_timestamp(history["points"][0]["temperatures"]["collected_at"]) == collected - timedelta(seconds=20)
            and history["points"][0]["gpus"][0]["temperature_celsius"] == 30.0,
            "CPU/SSD/GPU temperature round trip preserves true sampling timestamps")
    observed_frequency=node['metrics']['cpu_frequency']
    require(all(observed_frequency[key]==metrics['cpu_frequency'][key] for key in ('average_mhz','minimum_mhz','maximum_mhz','logical_cpus'))
            and parse_timestamp(observed_frequency['collected_at'])==collected-timedelta(seconds=20),
            'cached kernel CPU frequency keeps its range, logical CPU count and true sampling timestamp')
    require(node['metrics']['cpu_sockets']==metrics['cpu_sockets'],
            'per-socket CPU metrics retain physical IDs, independent utilization and unknown temperatures')
    original_packages=metrics['software_packages']
    metrics['software_packages']=[{'name':'obsolete-software','version':'old'}]
    metrics["collected_at"] = (collected - timedelta(minutes=5)).isoformat()
    metrics["cpu_usage_percent"] = 1.0
    metrics["platform_process"] = {"pid": 321, "cpu_percent_of_host": 0.75,
                                   "resident_memory_bytes": 2097152,
                                   "disk_read_bytes_per_second": 256.0,
                                   "disk_written_bytes_per_second": 96.0}
    metrics.pop("disk_collected_at")  # Old Agents omit this optional field.
    metrics.pop("block_devices")
    metrics.pop("overlay_networks")
    metrics.pop("temperatures")
    metrics.pop("cpu_frequency")
    metrics.pop("cpu_sockets")
    metrics.pop("platform_storage")  # Older Agents do not report the storage footprint.
    metrics["users"][0].pop("session_sources")  # Older Agent payloads remain accepted.
    metrics["users"][0].pop("session_evidence")
    metrics["users"][0].pop("account_status")
    report["agent_version"] = "0.0.0-delayed"
    report["inventory"] = inventory | {"hostname": "obsolete-hostname", "os_version": "obsolete-version"}
    require(request("/api/v1/agent/report", report, {"x-easymanage-agent-token": agent_token})[0] == 200,
            "delayed outbox report remains accepted")
    _, listing = request("/api/v1/nodes")
    node = next(item for item in listing["nodes"] if item["id"] == node_id)
    require(node["metrics"]["cpu_usage_percent"] == 37.5
            and parse_timestamp(node["metrics"]["collected_at"]) == collected
            and node['metrics']['software_packages']==original_packages,
            "delayed report never overwrites newer latest metrics")
    _, history = request(f"/api/v1/nodes/{node_id}/history")
    require(len(history["points"]) == 2 and history["points"][0]["cpu_usage_percent"] == 1.0
            and history["points"][0]["disk_collected_at"] is None
            and history["points"][1]["cpu_usage_percent"] == 37.5,
            "history remains chronological when delayed samples arrive")
    require(node["hostname"] == inventory["hostname"] and node["os_version"] == inventory["os_version"]
            and node["agent_version"] == "0.1.0", "delayed report never rolls back node inventory or Agent version")
    for offset,seconds,cpu,io in [(4,10,.25,100),(3,30,.75,300)]:
        precise=json.loads(json.dumps(report));precise['metrics']['collected_at']=(collected-timedelta(minutes=offset)).isoformat()
        precise['metrics']['platform_process']|={'sample_interval_seconds':seconds,'cpu_percent_of_host':cpu,'cpu_percent_of_one_core':cpu*4,
            'disk_read_bytes_per_second':io,'service':{'unit':'easymanage-agent.service','cpu_percent_of_one_core':cpu*8,'memory_current_bytes':4194304,
            'disk_read_bytes_per_second':io,'disk_written_bytes_per_second':0}}
        require(request('/api/v1/agent/report',precise,{'x-easymanage-agent-token':agent_token})[0]==200,'unequal-duration process report accepted')
    status, performance = request("/api/v1/platform-performance")
    agent_history = next(item for item in performance["agents"] if item["node_id"] == node_id)["stats"]
    agent_storage = next(item for item in performance["agents"] if item["node_id"] == node_id)["storage"]
    require(status == 200 and performance["period_seconds"] == 86400
            and performance["server"]["stats"]["sample_count"] >= 1
            and agent_history["sample_count"] == 4
            and agent_history["cpu_percent_of_host"]["samples"] == 4
            and abs(agent_history["cpu_percent_of_host"]["average"] - 0.5) < 0.000001
            and agent_history["cpu_percent_of_host"]["maximum"] == 0.75
            and agent_history["resident_memory_bytes"]["maximum"] == 2097152.0,
            "24-hour process statistics include controller and all real Agent report samples")
    require(abs(agent_history['cpu_percent_of_host']['time_weighted_average']-.625)<1e-9
            and agent_history['cpu_percent_of_host']['observed_seconds']==40
            and agent_history['cpu_percent_of_one_core']['time_weighted_average']==2.5
            and agent_history['service_cpu_percent_of_one_core']['time_weighted_average']==5
            and agent_history['disk_read_bytes_per_second']['total']==10000,
            'resource statistics weight real durations, preserve service scope and integrate observed I/O without filling gaps')
    require(performance["server"]["storage"]["total_bytes"]["samples"] >= 1
            and performance["server"]["storage"]["latest"]["database_bytes"] > 0
            and performance["server"]["storage"]["latest"]["web_bytes"] > 0
            and agent_storage["total_bytes"]["samples"] == 1
            and agent_storage["total_bytes"]["average"] == 1200.0
            and agent_storage["latest"]["state_bytes"] == 200,
            "hourly storage footprint samples include the dedicated database and deduplicate cached Agent reports")
    status, target_list = request("/api/v1/operations/targets")
    target = target_list["targets"][0]
    require(status == 200 and target["node_id"] == node_id and target["elevation_available"]
            and target["update_state"] == "not_published" and not target["restart_available"]
            and set(target) == {"node_id", "hostname", "host", "port", "default_user", "elevation_available", "update_available", "update_state", "restart_available"},
            "target listing exposes no credentials or private deployment paths")
    status, platform_health = request("/api/v1/platform-health")
    require(status == 200 and platform_health["platform"]["database_healthy"]
            and platform_health["nodes"][0]["node_id"] == node_id
            and not platform_health["nodes"][0]["monitoring_paused"]
            and platform_health["sample_policy"]["server_process_seconds"] > 0
            and platform_health["platform_to_nodes"][0]["node_id"] == node_id,
            "platform health has its own endpoint with database and trusted topology")
    status, gitea = request("/api/v1/gitea")
    require(status == 200 and not gitea["healthy"] and gitea["version"] is None,
            "unavailable private-namespace Gitea reports an honest degraded state")
    require(request(f"/api/v1/nodes/{node_id}/elevate", {"username": "smoke", "password": "not-a-real-password"},
                    {"Origin": "https://cross-origin.invalid"})[0] == 403,
            "cross-origin elevation is rejected before SSH")
    require(request(f"/api/v1/nodes/{node_id}/users/manage", {"action": "delete", "subject": "smoke",
                    "confirmation": "smoke", "username": "smoke", "password": "not-a-real-password"},
                    {"Origin": "https://cross-origin.invalid"})[0] == 403,
            "cross-origin account mutation is rejected before SSH")
    require(request(f"/api/v1/nodes/{node_id}/agent/update", {},
                    {"Origin": "https://cross-origin.invalid"})[0] == 403,
            "cross-origin Agent deployment is rejected before SSH")
    require(request(f"/api/v1/nodes/{node_id}/agent/restart", {},
                    {"Origin": "https://cross-origin.invalid"})[0] == 403,
            "cross-origin Agent restart is rejected before SSH")
    require(request(f"/api/v1/nodes/{node_id}/admin-credentials", {"username":"smoke","password":"fixture-secret-not-for-response"},
                    {"Origin":"https://cross-origin.invalid"})[0] == 403,
            "cross-origin administrator credential storage is rejected before SSH or vault writes")
    require(request(f"/api/v1/nodes/{node_id}/admin-credentials")[1] == {"available":False},
            "administrator credential status exposes only authorization availability, never passwords or keys")
    require(request("/api/v1/platform/update", {"username":"smoke","password":"fixture-only","candidate_sha256":"a"*64},
                    {"Origin":"https://cross-origin.invalid"})[0] == 403,
            "cross-origin platform update is rejected before SSH or installation")
    require(request("/api/v1/platform/update", {"username":"-invalid","password":"fixture-only","candidate_sha256":"a"*64},
                    {"Origin":base_url})[0] == 400,
            "invalid platform update credentials are rejected before SSH")
    status, candidate = request("/api/v1/platform/update")
    require(status == 200 and candidate["update_available"] is False and candidate["supported"] is False,
            "unconfigured platform update is visible but safely unavailable")
    for endpoint in (f"/api/v1/nodes/{node_id}/agent/force-restart", "/api/v1/platform/force-restart"):
        require(request(endpoint, {"username":"smoke", "password":"not-a-real-password"},
                        {"Origin":"https://cross-origin.invalid"})[0] == 403,
                f"cross-origin force restart blocked: {endpoint}")
    require(request("/api/v1/operations/recovery")[1]["database_healthy"],
            "database-independent recovery endpoint reports healthy database")
    require(request(f"/api/v1/nodes/{node_id}/agent/force-restart",
                    {"username":"bad;user", "password":"not-a-real-password"},
                    {"Origin":base_url})[0] == 400, "invalid emergency credentials rejected before SSH")
    status, guide = request("/guide/")
    require(status == 200 and "isolated-smoke-vitepress" in guide, "built documentation is served from the same origin")
    with opener.open(base_url + "/guide/", timeout=12) as response:
        require(response.headers.get("x-frame-options") == "SAMEORIGIN"
                and "frame-ancestors 'self'" in response.headers.get("content-security-policy", "")
                and "script-src 'self' 'unsafe-inline'" in response.headers.get("content-security-policy", ""),
                "VitePress may be embedded only by the same-origin dashboard")
    with opener.open(base_url + "/", timeout=12) as response:
        require(response.headers.get("x-frame-options") == "DENY"
                and "frame-ancestors 'none'" in response.headers.get("content-security-policy", ""),
                "dashboard retains strict anti-framing headers")
    require(request("/guide/assets/app.js")[0] == 200, "documentation assets are served under the configured base")
    status, platform_docs = request("/guide/easymanage.html")
    require(status == 200 and "isolated-smoke-platform-docs" in platform_docs,
            "independent VitePress platform documentation is served")
    require(request("/manual/")[1] == guide, "previous manual URL redirects to VitePress manual")
    require(request("/api/v1/nodes")[0] == 200, "documentation requests do not break monitoring")

    # Synthetic permission-review fixtures run only in this runner's guarded sandbox.
    permission = {"category": "home", "title": "smoke", "path": "/home/smoke", "status": "warning",
                  "observed": "UID 1000 · GID 1000 · 2770", "expected": "0700", "detail": "group access requires review"}
    review_report = json.loads(json.dumps(report))
    review_report["inventory"] = inventory
    review_report["agent_version"] = "0.1.0"
    review_report["metrics"]["permission_audit"] = {"collected_at": datetime.now(timezone.utc).isoformat(),
                                                   "checks": [dict(permission)], "truncated": False}

    def send_permission_report():
        review_report["metrics"]["collected_at"] = datetime.now(timezone.utc).isoformat()
        require(request("/api/v1/agent/report", review_report, {"x-easymanage-agent-token": agent_token})[0] == 200,
                "permission fixture report accepted")

    send_permission_report()
    review_path = f"/api/v1/nodes/{node_id}/permission-review"
    mark = {"check": permission, "accepted": True, "note": "isolated operator confirmation"}
    origin = {"Origin": base_url, "Sec-Fetch-Site": "same-origin"}
    require(request(review_path, mark, {"Origin": "https://cross-origin.invalid"})[0] == 403,
            "cross-origin manual annotation rejected")
    require(request(review_path, mark | {"check": permission | {"observed": "forged"}}, origin)[0] == 409,
            "client cannot mark unreported evidence")
    require(request(review_path, mark | {"note": "x" * 1001}, origin)[0] == 400, "review note length is bounded")
    require(request(review_path, mark, origin)[0] == 200, "same-origin current evidence can be manually accepted")
    _, reviews = request("/api/v1/permission-reviews")
    require(len(reviews["reviews"]) == 1 and reviews["reviews"][0]["check"] == permission
            and reviews["reviews"][0]["note"] == mark["note"], "manual annotation persists independently of browser state")
    _, reviewed_nodes = request("/api/v1/nodes")
    require(reviewed_nodes["nodes"][0]["metrics"]["permission_audit"]["checks"][0]["status"] == "warning",
            "manual acceptance does not rewrite automated evidence")
    review_report["metrics"]["permission_audit"]["collected_at"] = datetime.now(timezone.utc).isoformat()
    send_permission_report()
    require(len(request("/api/v1/permission-reviews")[1]["reviews"]) == 1,
            "unchanged evidence retains confirmation across repeated collection")
    delayed_review = json.loads(json.dumps(review_report))
    delayed_review["metrics"]["collected_at"] = (datetime.now(timezone.utc) - timedelta(minutes=5)).isoformat()
    delayed_review["metrics"]["permission_audit"]["checks"][0]["observed"] = "old changed evidence"
    require(request("/api/v1/agent/report", delayed_review, {"x-easymanage-agent-token": agent_token})[0] == 200
            and len(request("/api/v1/permission-reviews")[1]["reviews"]) == 1,
            "delayed report cannot invalidate confirmation of the current snapshot")
    review_report["metrics"]["permission_audit"]["checks"][0]["observed"] = "0777"
    send_permission_report()
    require(request("/api/v1/permission-reviews")[1]["reviews"] == [], "changed permission evidence invalidates acceptance")
    require(request(review_path, mark, origin)[0] == 409, "stale browser cannot reaccept previous evidence")
    review_report["metrics"]["permission_audit"]["checks"] = [dict(permission)]
    send_permission_report()
    require(request("/api/v1/permission-reviews")[1]["reviews"] == [], "reverted evidence does not revive an invalidated acceptance")
    review_report["metrics"]["permission_audit"]["collected_at"] = (datetime.now(timezone.utc) - timedelta(minutes=9)).isoformat()
    send_permission_report()
    require(request(review_path, mark, origin)[0] == 409, "expired permission snapshot cannot be manually accepted")
    review_report["metrics"]["permission_audit"]["collected_at"] = datetime.now(timezone.utc).isoformat()
    send_permission_report()
    require(request(review_path, mark, origin)[0] == 200
            and request(review_path, mark | {"accepted": False}, origin)[0] == 200
            and request("/api/v1/permission-reviews")[1]["reviews"] == [], "manual annotation can be explicitly revoked")

    _, rotated = request("/api/v1/agent/enroll", enrollment, {"x-easymanage-enrollment-token": enrollment_token})
    require(rotated["agent_token"] != agent_token
            and request("/api/v1/agent/report", report, {"x-easymanage-agent-token": agent_token})[0] == 401,
            "re-enrollment invalidates the previous Agent credential")

    status, settings = request("/api/v1/collection-settings")
    require(status == 200 and len(settings["items"]) == 10, "all collection groups have bounded configurable intervals")
    original = settings["policy"]
    require(request("/api/v1/collection-settings", original, {"Origin": "https://other.invalid"})[0] == 403,
            "cross-origin collection changes are rejected")
    require(request("/api/v1/collection-settings", original | {"permission_seconds": 1}, origin)[0] == 400,
            "expensive permission scans cannot be configured at one-second intervals")
    require(request("/api/v1/collection-settings", original | {"live_seconds": 1.5}, origin)[0] == 422,
            "fractional collection intervals are rejected")
    require(request("/api/v1/collection-settings", original | {"arbitrary_command": "no"}, origin)[0] == 422,
            "settings cannot contain commands or unknown fields")
    changed = original | {"live_seconds": 2, "gpu_seconds": 30, "disk_seconds": 60,
                          "extended_seconds": 120, "server_process_seconds": 10}
    status, saved = request("/api/v1/collection-settings", changed, origin)
    require(status == 200 and saved["policy"]["revision"] == original["revision"] + 1,
            "same-origin settings persist with optimistic concurrency and a new revision")
    require(request("/api/v1/collection-settings", original, origin)[0] == 409,
            "stale settings pages cannot overwrite a newer revision")
    require(request("/api/v1/collection-settings")[1]["policy"] == saved["policy"], "saved settings are shared across browsers")
    require(request("/api/v1/collection-settings", original | {"revision": saved["policy"]["revision"]}, origin)[0] == 200,
            "original collection intervals can be safely restored")
    path=f"/api/v1/nodes/{node_id}/user-collection"
    change={"enabled":False,"expected_enabled":True}
    require(request(path,change,{"Origin":"https://other.invalid"})[0]==403,
            "cross-origin user collection changes are rejected")
    require(request(f"/api/v1/nodes/{uuid.uuid4()}/user-collection",change,origin)[0]==404,
            "user collection settings cannot create phantom machines")
    require(request(path,change|{"enabled":"false"},origin)[0]==422,
            "user collection switches require real boolean values")
    require(request(path,change,origin)[0]==200 and request(path,change,origin)[0]==409,
            "per-node user collection changes persist and reject stale pages")
    current=request('/api/v1/collection-settings')[1]
    require(next(a for a in current['agents'] if a['node_id']==node_id)['users_enabled'] is False,
            "per-node switches are shared across browsers independently of global intervals")
    require(request('/api/v1/collection-settings',current['policy']|{'users_enabled':False},origin)[0]==400,
            "global interval settings cannot override per-node user switches")
    token_header={"x-easymanage-agent-token":rotated['agent_token']}
    require('users_enabled' not in request('/api/v1/agent/report',report,token_header)[1]['collection_policy'],
            "legacy Agents receive their original policy shape even when user collection is disabled")
    capable=report|{'metrics':report['metrics']|{'collection_policy':current['policy']|{'users_enabled':True}}}
    require(request('/api/v1/agent/report',capable,token_header)[1]['collection_policy']['users_enabled'] is False,
            "capable Agents receive the node-specific disabled switch in authenticated report responses")
    permission_path=f'/api/v1/nodes/{node_id}/permission-collection'
    require(request(permission_path,change,{'Origin':'https://other.invalid'})[0]==403
            and request(permission_path,change|{'enabled':'false'},origin)[0]==422,
            'permission collection requires same origin and a real boolean')
    require(request(f'/api/v1/nodes/{uuid.uuid4()}/permission-collection',change,origin)[0]==404,
            'permission collection cannot create phantom nodes')
    fresh=json.loads(json.dumps(review_report))
    fresh_at=datetime.now(timezone.utc)+timedelta(seconds=1)
    fresh['metrics']['collected_at']=fresh_at.isoformat()
    fresh['metrics']['permission_audit']['collected_at']=datetime.now(timezone.utc).isoformat()
    fresh['metrics']['collection_policy']=current['policy']|{'users_enabled':True,'permissions_enabled':True}
    require(request('/api/v1/agent/report',fresh,token_header)[0]==200
            and request(review_path,mark,origin)[0]==200,'fresh permission evidence can be reviewed before pausing')
    require(request(permission_path,change,origin)[0]==200 and request(permission_path,change,origin)[0]==409,
            'permission switches persist and reject stale pages')
    settings=request('/api/v1/collection-settings')[1]
    own=next(a for a in settings['agents'] if a['node_id']==node_id)
    require(own['permissions_enabled'] is False and own['users_enabled'] is False,
            'permission and user switches remain independent')
    require(request('/api/v1/collection-settings',settings['policy']|{'permissions_enabled':False},origin)[0]==400,
            'global intervals cannot overwrite per-node permission switches')
    require('permissions_enabled' not in request('/api/v1/agent/report',capable,token_header)[1]['collection_policy'],
            'user-switch-only Agents do not receive unsupported permission policy fields')
    paused=json.loads(json.dumps(fresh))
    paused['metrics']['collected_at']=(fresh_at+timedelta(seconds=1)).isoformat()
    paused['metrics']['permission_audit']=None
    paused['metrics']['collection_policy']['permissions_enabled']=False
    response=request('/api/v1/agent/report',paused,token_header)
    require(response[0]==200 and response[1]['collection_policy']['permissions_enabled'] is False
            and len(request('/api/v1/permission-reviews')[1]['reviews'])==1,
            'paused permission reports preserve dormant manual reviews without inventing new evidence')
    require(request(review_path,mark,origin)[0]==409,'paused snapshots cannot receive new manual OK marks')
    require(request(permission_path,{'enabled':True,'expected_enabled':False},origin)[0]==200,
            'permission collection can be re-enabled independently')
    fresh['metrics']['collected_at']=(fresh_at+timedelta(seconds=2)).isoformat()
    fresh['metrics']['permission_audit']['checks'][0]['observed']='changed after pause'
    require(request('/api/v1/agent/report',fresh,token_header)[0]==200
            and request('/api/v1/permission-reviews')[1]['reviews']==[],
            'fresh changed evidence after resume invalidates the previous manual OK')
    require(request(permission_path,change,origin)[0]==200,'permission switch restored to disabled for persistence tests')
    proxy_path=f'/api/v1/nodes/{node_id}/proxy-collection'
    require(request(proxy_path,change,{'Origin':'https://other.invalid'})[0]==403
            and request(proxy_path,change|{'enabled':'false'},origin)[0]==422,
            'proxy collection switches require same origin and a real boolean')
    require(request(f'/api/v1/nodes/{uuid.uuid4()}/proxy-collection',change,origin)[0]==404,
            'proxy collection switch cannot create phantom nodes')
    require(request(proxy_path,change,origin)[0]==200 and request(proxy_path,change,origin)[0]==409,
            'per-node proxy switch persists and rejects stale browser writes')
    settings=request('/api/v1/collection-settings')[1]
    own=next(a for a in settings['agents'] if a['node_id']==node_id)
    require(own['proxy_enabled'] is False and own['users_enabled'] is False and own['permissions_enabled'] is False,
            'proxy control does not overwrite independent user or permission switches')
    require(request('/api/v1/collection-settings',settings['policy']|{'proxy_enabled':False},origin)[0]==400,
            'global policy cannot override per-machine proxy switches')
    require('proxy_enabled' not in request('/api/v1/agent/report',fresh,token_header)[1]['collection_policy'],
            'previous Agents never receive unsupported proxy fields in strict collection policies')
    proxy_capable=report|{'metrics':report['metrics']|{'collection_policy':settings['policy']|{'proxy_enabled':True}}}
    require(request('/api/v1/agent/report',proxy_capable,token_header)[1]['collection_policy']['proxy_enabled'] is False,
            'new Agent reports advertise proxy capability and receive their node-specific setting')
    pause_path=f'/api/v1/machines/{node_id}/monitoring'
    pause={'confirmation':node_id,'paused':True,'expected_revision':0}
    require(request(pause_path,pause,{'Origin':'https://other.invalid'})[0]==403
            and request(pause_path,pause|{'confirmation':str(uuid.uuid4())},origin)[0]==400,
            'monitoring pause requires same origin and the exact existing node ID')
    require(request(f'/api/v1/machines/{uuid.uuid4()}/monitoring',pause,origin)[0]==400
            and request(pause_path,pause|{'paused':'true'},origin)[0]==422,
            'monitoring controls reject mismatched identities and non-boolean intent')
    require(request(pause_path,pause,origin)[0]==200 and request(pause_path,pause,origin)[0]==409,
            'monitoring pause is revisioned and rejects stale browser writes')
    managed=next(m for m in request('/api/v1/machines')[1]['machines'] if m['node_id']==node_id)
    require(managed['monitoring_paused'] and not managed['monitoring_supported']
            and managed['monitoring_applied_revision'] is None and not managed['redeploy_available'],
            'legacy Agent pause stays pending, never falsely confirmed or offered redeployment')
    control={'node_id':node_id,'applied':{'revision':0,'paused':False}}
    require(request('/api/v1/agent/control',control)[0]==401
            and request('/api/v1/agent/control',control,{'x-easymanage-agent-token':'invalid'})[0]==401,
            'lightweight control requires the original authenticated Agent identity')
    require(request('/api/v1/agent/control',control,token_header)[1]['control']=={'revision':1,'paused':True},
            'authenticated lightweight control receives saved pause intent without a metrics sample')
    acknowledged=control|{'applied':{'revision':1,'paused':True}}
    require(request('/api/v1/agent/control',acknowledged,token_header)[0]==200,
            'Agent can acknowledge pause without sending fabricated metrics')
    require('proxy_enabled' not in request('/api/v1/agent/control',acknowledged,token_header)[1]['collection_policy']
            and request('/api/v1/agent/control',acknowledged|{'supports_proxy_collection':True},token_header)[1]['collection_policy']['proxy_enabled'] is False,
            'paused control polls negotiate proxy support without breaking previous Agent versions')
    unknown=str(uuid.uuid4())
    require(request(f'/api/v1/machines/{unknown}/monitoring',pause|{'confirmation':unknown},origin)[0]==404,
            'monitoring pause cannot create nonexistent machines')
    require('agent_control' not in request('/api/v1/agent/report',report,token_header)[1],
            'legacy report responses do not acquire unsupported monitoring control fields')
    legacy_id=str(uuid.uuid4())
    _,legacy_identity=request('/api/v1/agent/enroll',enrollment|{'requested_node_id':legacy_id},
        {'x-easymanage-enrollment-token':enrollment_token})
    legacy_header={'x-easymanage-agent-token':legacy_identity['agent_token']}
    legacy_report=report|{'node_id':legacy_id,'metrics':report['metrics']|{'collected_at':datetime.now(timezone.utc).isoformat()}}
    require(request('/api/v1/agent/report',legacy_report,legacy_header)[0]==200,
            'legacy resume fixture has a real authenticated report without control capability')
    legacy_path=f'/api/v1/machines/{legacy_id}/monitoring'
    request(legacy_path,{'confirmation':legacy_id,'paused':True,'expected_revision':0},origin)
    require(not next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==legacy_id)['online'],
            'legacy Agent remains hidden while monitoring pause is requested')
    request(legacy_path,{'confirmation':legacy_id,'paused':False,'expected_revision':1},origin)
    legacy_report['metrics']['collected_at']=datetime.now(timezone.utc).isoformat()
    request('/api/v1/agent/report',legacy_report,legacy_header)
    legacy_node=next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==legacy_id)
    require(legacy_node['online'] and legacy_node['monitoring_applied_revision'] is None,
            'resumed legacy Agent uses real fresh heartbeats without fabricating a control acknowledgement')
    legacy_live=live|{'node_id':legacy_id}
    legacy_live['metrics']=live['metrics']|{'collected_at':datetime.now(timezone.utc).isoformat()}
    require(request('/api/v1/agent/live',legacy_live,legacy_header)[0]==204
            and any(n['node_id']==legacy_id for n in request('/api/v1/nodes/live')[1]['nodes']),
            'resumed legacy Agent live metrics are visible without unsupported control fields')
    resume_id=str(uuid.uuid4())
    _,resume_identity=request('/api/v1/agent/enroll',enrollment|{'requested_node_id':resume_id},
        {'x-easymanage-enrollment-token':enrollment_token})
    resume_header={'x-easymanage-agent-token':resume_identity['agent_token']}
    current_report=report|{'node_id':resume_id,'metrics':report['metrics']|{
        'collected_at':datetime.now(timezone.utc).isoformat(),'agent_control':{'revision':0,'paused':False}}}
    require(request('/api/v1/agent/report',current_report,resume_header)[0]==200,
            'capable monitoring Agent initially reports under its own identity')
    resume_path=f'/api/v1/machines/{resume_id}/monitoring'
    request(resume_path,{'confirmation':resume_id,'paused':True,'expected_revision':0},origin)
    request('/api/v1/agent/control',{'node_id':resume_id,'applied':{'revision':1,'paused':True}},resume_header)
    request(resume_path,{'confirmation':resume_id,'paused':False,'expected_revision':1},origin)
    request('/api/v1/agent/control',{'node_id':resume_id,'applied':{'revision':2,'paused':False}},resume_header)
    resume_node=next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==resume_id)
    require(not resume_node['online'] and resume_node['monitoring_applied_revision']==2,
            'resume acknowledgement alone does not turn old metrics into a new online heartbeat')
    current_report['metrics']['agent_control']={'revision':2,'paused':False}
    current_report['metrics']['collected_at']=datetime.now(timezone.utc).isoformat()
    require(request('/api/v1/agent/report',current_report,resume_header)[0]==200
            and next(n for n in request('/api/v1/nodes')[1]['nodes'] if n['id']==resume_id)['online'],
            'resumed monitoring is online only after a fresh report with the applied revision')


def exercise_agent(binary, root, environment, base_url, ca_certificate, enrollment_token):
    """Exercise the real collector only inside the guarded private namespaces."""
    agent = proxy = None
    agent_root = root / "real-agent"
    agent_root.mkdir(mode=0o700)
    agent_log = (agent_root / "agent.log").open("w+")
    configuration = agent_root / "agent.env"
    configuration.write_text("")
    configuration.chmod(0o600)
    state = agent_root / "state/state.db"
    node_id = str(uuid.uuid4())
    env = environment | {
        "EASYMANAGE_ENV_FILE": str(configuration), "EASYMANAGE_SERVER_URL": base_url,
        "EASYMANAGE_CA_CERTIFICATE": str(ca_certificate), "EASYMANAGE_AGENT_STATE_DB": str(state),
        "EASYMANAGE_ENROLLMENT_TOKEN": enrollment_token, "EASYMANAGE_NODE_ID": node_id,
        "EASYMANAGE_REPORT_INTERVAL_SECONDS": "5", "EASYMANAGE_REQUEST_TIMEOUT_SECONDS": "3",
        "TOKIO_WORKER_THREADS": "2", "RUST_LOG": "warn",
    }
    context = ssl.create_default_context(cafile=str(ca_certificate))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))

    def listing():
        with opener.open(base_url + "/api/v1/nodes", timeout=3) as response:
            return json.load(response)["nodes"]

    def live_listing():
        with opener.open(base_url + "/api/v1/nodes/live", timeout=3) as response:
            return json.load(response)["nodes"]

    def wait_for_heartbeat(started):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            if agent.poll() is not None:
                raise AssertionError("The isolated Agent exited before its first heartbeat")
            for node in listing():
                metrics = node.get("metrics")
                if node["id"] == node_id and node["online"] and metrics:
                    at = parse_timestamp(metrics["collected_at"])
                    if at >= started:
                        return node
            time.sleep(0.2)
        raise AssertionError("The isolated real Agent did not report within 45 seconds")

    def graceful_shutdown():
        agent.send_signal(signal.SIGTERM)
        require(agent.wait(timeout=8) == 0, "real Agent exits gracefully on SIGTERM")

    def persisted_identity():
        with sqlite3.connect("file:" + str(state) + "?mode=ro", uri=True) as connection:
            return dict(connection.execute("SELECT key, value FROM metadata WHERE key IN ('node_id', 'agent_token')"))

    before = {node["id"] for node in listing()}
    try:
        # Named synthetic proxy: private loopback only, never real proxy traffic.
        marker=agent_root/'proxy-ready'
        proxy_script="import ctypes,socket,time,sys;from pathlib import Path;ctypes.CDLL(None).prctl(15,b'mihomo',0,0,0);s=socket.socket();s.bind(('127.0.0.1',0));s.listen();Path(sys.argv[1]).write_text('ready');time.sleep(600)"
        proxy=subprocess.Popen(['/usr/bin/python3','-c',proxy_script,str(marker)],env=environment,
            stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        deadline=time.monotonic()+5
        while not marker.exists() and time.monotonic()<deadline:time.sleep(.05)
        require(marker.exists() and proxy.poll() is None,'synthetic proxy listens only inside the isolated loopback namespace')
        started = datetime.now(timezone.utc)
        agent = subprocess.Popen([str(binary.resolve())], cwd=agent_root, env=env, stdout=agent_log, stderr=subprocess.STDOUT)
        node = wait_for_heartbeat(started)
        metrics = node["metrics"]
        cpu_blocks=[dict((k.strip(),v.strip()) for line in block.splitlines() for k,sep,v in [line.partition(':')] if sep) for block in Path('/proc/cpuinfo').read_text().split('\n\n')]
        cpu_blocks=[block for block in cpu_blocks if 'processor' in block]
        if cpu_blocks and all(block.get('physical id','').isdigit() for block in cpu_blocks):
            expected=sorted({int(block['physical id']) for block in cpu_blocks})
            sockets=metrics.get('cpu_sockets',[])
            require([cpu['id'] for cpu in sockets]==expected and sum(cpu['logical_cpus'] for cpu in sockets)==len(cpu_blocks)
                    and all(cpu['usage_percent'] is not None and 0<=cpu['usage_percent']<=100 for cpu in sockets),
                    'real collector identifies physical CPU packages and aggregates existing per-core usage without counting threads as CPUs')
        require(any(p['pid']==proxy.pid and p['name']=='mihomo' and p['listen_addresses'] for p in metrics['proxy_services']),
                'enabled proxy monitoring sees a real synthetic proxy process and its private listening socket')
        status_fields = dict(line.split(":", 1) for line in Path(f"/proc/{agent.pid}/status").read_text().splitlines() if ":" in line)
        threads = int(status_fields["Threads"].strip())
        require(threads <= 12, f"real Agent keeps a small runtime thread count ({threads}), independent of host CPU cores")
        require(metrics["privilege"]["mode"] == "basic" and metrics["privilege"]["uid"] == os.geteuid() != 0,
                "real isolated Agent reports its actual non-root Basic identity")
        live_deadline = time.monotonic() + 8
        live_times = set()
        while time.monotonic() < live_deadline and len(live_times) < 2:
            live_times.update(item["received_at"] for item in live_listing() if item["node_id"] == node_id)
            time.sleep(0.2)
        require(len(live_times) >= 2, "real Agent advances lightweight live metrics at one-second cadence")
        require(all(isinstance(metrics.get(key), list) for key in ("disks", "block_devices", "networks", "gpus", "users", "interfaces", "routes", "proxy_services", "collection_issues"))
                and metrics.get("inventory_collected_at") is not None
                and metrics.get("disk_collected_at") is not None and metrics["memory_total_bytes"] > 0,
                "real collector publishes metrics and user/network/storage inventory schema")
        footprint = metrics.get("platform_storage") or {}
        hardware = metrics.get("hardware_details") or {}
        require(bool(hardware.get("processors")) and bool(hardware.get("memory"))
                and all(isinstance(hardware.get(key), list) for key in ("motherboard", "graphics", "displays", "disks", "audio", "network", "notes"))
                and hardware.get("collected_at") is not None,
                "real Agent reports daily hardware schema with honest missing-device notes")
        require(footprint.get("binary_bytes", 0) > 0 and footprint.get("state_bytes", 0) > 0
                and footprint.get("total_bytes") == footprint["binary_bytes"] + footprint["state_bytes"]
                and footprint.get("database_bytes") is None and footprint.get("web_bytes") is None,
                "real Agent counts only its executable and private SQLite files")
        first_metric_at = parse_timestamp(metrics["collected_at"])
        first_disk_at = parse_timestamp(metrics["disk_collected_at"])
        first_temperature_at = parse_timestamp(metrics["temperatures"]["collected_at"])
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            updated = next((item for item in listing() if item["id"] == node_id), None)
            if updated and parse_timestamp(updated["metrics"]["collected_at"]) > first_metric_at:
                break
            time.sleep(0.2)
        else:
            raise AssertionError("The isolated Agent did not send a second report")
        require(parse_timestamp(updated["metrics"]["temperatures"]["collected_at"]) == first_temperature_at,
                "CPU/SSD temperature cache avoids polling sensors on each heartbeat")
        require(parse_timestamp(updated["metrics"]["disk_collected_at"]) == first_disk_at,
                "disk probes are cached while ordinary metrics keep advancing")
        require(updated["metrics"]["hardware_details"] == hardware,
                "daily hardware details are cached across heartbeats")
        require(any(user["username"] == "root" for user in metrics["users"]), "real user inventory includes root")
        require(all(isinstance(user.get("session_evidence"), dict)
                    and set(user["session_evidence"]) == {"terminal_records", "logind_sessions", "ssh_processes"}
                    and all(value is None or isinstance(value, int) and value >= 0
                            for value in user["session_evidence"].values())
                    for user in metrics["users"]),
                "real Agent reports bounded per-source login evidence")
        require(isinstance(metrics.get("host_security"), dict)
                and set(metrics["host_security"]) == {"apparmor_enabled", "aslr_mode", "ptrace_scope", "kernel_lockdown", "collected_at", "interval_seconds"}
                and metrics['host_security']['interval_seconds']==3600,
                "real Agent reports bounded host security observations without inferring unknowns")
        require(updated['metrics']['host_security']==metrics['host_security'],
                'hourly security snapshots retain their actual sample timestamp across heartbeats')
        # Simulate a restored database losing just the negotiated inventory cache.
        # The real Agent must resend its full durable report, not re-enroll or
        # silently turn a missing list into an empty one. Private DB only.
        psql=['/usr/lib/postgresql/18/bin/psql','-X','-A','-t','-h','127.0.0.1','-p','55432','-U','smoke','-d','easymanage_smoke','-v','ON_ERROR_STOP=1','-c']
        deleted=subprocess.run(psql+[f"DELETE FROM node_software_inventory WHERE node_id='{node_id}'"],env=environment,capture_output=True,text=True,timeout=5)
        require(deleted.returncode==0,'private software-cache loss fixture created')
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            exists=subprocess.run(psql+[f"SELECT COUNT(*) FROM node_software_inventory WHERE node_id='{node_id}'"],env=environment,capture_output=True,text=True,timeout=5)
            if exists.returncode==0 and exists.stdout.strip()=='1':break
            time.sleep(.3)
        else:raise AssertionError('Real Agent did not restore missing software inventory')
        require(True,'real Agent restores a lost software cache with a full retry while preserving its identity and buffered history')
        tools = metrics.get("ai_tools")
        require(isinstance(tools, list) and {"codex", "claude", "gemini", "opencode", "aider"} == {item["name"] for item in tools}
                and all(item["category"] == "assistant" and set(item) == {"name", "category", "executable_paths", "processes", "service_units"} for item in tools),
                "real Agent reports only named Agent tools, without runtimes, credentials or command lines")
        programming = metrics.get("programming_tools")
        require(isinstance(programming, list) and {"rustc", "cargo", "gcc", "python3", "node", "git", "nvidia-smi", "matlab", "mex", "mcc", "nvcc", "hipcc"}.issubset({item["name"] for item in programming})
                and all(any(item["category"] == category for item in programming) for category in ("GPU", "MATLAB", "C++(GPU)"))
                and all(set(item) == {"name", "category", "executable_paths", "package_version"} for item in programming),
                "real Agent reports bounded programming tools without executing binaries")
        with opener.open(base_url + "/api/v1/collection-settings", timeout=3) as response:
            original_policy = json.load(response)["policy"]
        changed = original_policy | {"live_seconds": 2, "report_seconds": 5,
                                    "gpu_seconds": 30, "disk_seconds": 60, "extended_seconds": 120}
        query = urllib.request.Request(base_url + "/api/v1/collection-settings", data=json.dumps(changed).encode(),
                                       headers={"Content-Type": "application/json", "Origin": base_url})
        with opener.open(query, timeout=3) as response:
            expected_policy = json.load(response)["policy"]
        pid_before = agent.pid
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            applied = next((node for node in listing() if node["id"] == node_id), None)
            if applied and applied["metrics"].get("collection_policy") == expected_policy | {"users_enabled": True,"permissions_enabled":True,"proxy_enabled":True}:
                break
            time.sleep(0.2)
        else:
            raise AssertionError("Agent did not apply collection settings")
        require(agent.pid == pid_before and agent.poll() is None
                and applied["metrics"]["privilege"]["mode"] == "basic",
                "running Agent applies the exact policy without restart or privilege changes")
        received = set()
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline and len(received) < 3:
            received.update(parse_timestamp(item["received_at"]) for item in live_listing() if item["node_id"] == node_id)
            time.sleep(0.1)
        ordered = sorted(received)
        require(len(ordered) >= 3 and all(1.5 <= (right-left).total_seconds() <= 2.5 for left,right in zip(ordered,ordered[1:])),
                "real live report cadence changes to the configured two seconds")
        newer = next(node for node in listing() if node["id"] == node_id)
        require(newer["metrics"]["gpu_collected_at"] == applied["metrics"]["gpu_collected_at"]
                and newer["metrics"]["disk_collected_at"] == applied["metrics"]["disk_collected_at"]
                and newer["metrics"]["inventory_collected_at"] == applied["metrics"]["inventory_collected_at"]
                and newer["metrics"]["platform_storage"]["collected_at"] == applied["metrics"]["platform_storage"]["collected_at"],
                "slow probes retain real sampling timestamps instead of inventing fresh samples")
        activity=newer['metrics'].get('network_activity')
        groups={g['group']:g for g in (activity or {}).get('groups',[])}
        require(activity and activity['process_id']==agent.pid and set(groups)=={'agent_enroll','agent_report','agent_live','agent_control'}
                and groups['agent_report']['started']>0 and groups['agent_live']['started']>0
                and groups['agent_report']['submitted_bytes']>0 and groups['agent_report']['received_bytes']>0,
                'real Agent reports only its own counted HTTP attempts and payload budget, not host NIC traffic')
        with opener.open(base_url+'/api/v1/platform-network',timeout=3) as response:
            network=json.load(response)
        require(next(a for a in network['agents'] if a['node_id']==node_id)['activity']==activity,
                'platform network view exposes the latest authenticated Agent counters')
        def switch_users(enabled, expected):
            query=urllib.request.Request(base_url+f'/api/v1/nodes/{node_id}/user-collection',
                data=json.dumps({'enabled':enabled,'expected_enabled':expected}).encode(),
                headers={'Content-Type':'application/json','Origin':base_url})
            with opener.open(query,timeout=3) as response:
                require(json.load(response)['enabled'] is enabled,'per-node switch saved through real HTTPS')
        def wait_for_user_switch(enabled):
            deadline=time.monotonic()+35
            while time.monotonic()<deadline:
                observed=next((n for n in listing() if n['id']==node_id),None)
                if observed and observed['metrics'].get('collection_policy',{}).get('users_enabled') is enabled:
                    if not enabled or observed['metrics']['users']:
                        return observed
                time.sleep(.2)
            raise AssertionError('Agent did not acknowledge the user collection switch')
        switch_users(False,True)
        disabled=wait_for_user_switch(False)
        require(disabled['metrics']['users']==[] and disabled['metrics']['permission_audit']
                and disabled['metrics']['privilege']['mode']=='basic' and agent.pid==pid_before,
                'running Agent stops user reports without stopping permissions, metrics or changing privileges')
        def switch_permissions(enabled,expected):
            query=urllib.request.Request(base_url+f'/api/v1/nodes/{node_id}/permission-collection',
                data=json.dumps({'enabled':enabled,'expected_enabled':expected}).encode(),
                headers={'Content-Type':'application/json','Origin':base_url})
            with opener.open(query,timeout=3) as response:
                require(json.load(response)['enabled'] is enabled,'permission collection switch saved through real HTTPS')
        def wait_for_permission_switch(enabled):
            deadline=time.monotonic()+35
            while time.monotonic()<deadline:
                observed=next((n for n in listing() if n['id']==node_id),None)
                if observed and observed['metrics'].get('collection_policy',{}).get('permissions_enabled') is enabled:
                    if not enabled or observed['metrics'].get('permission_audit'):
                        return observed
                time.sleep(.2)
            raise AssertionError('Agent did not acknowledge the permission collection switch')
        switch_permissions(False,True)
        paused=wait_for_permission_switch(False)
        require(paused['metrics']['permission_audit'] is None and agent.pid==pid_before and agent.poll() is None,
                'real Agent stops permission collection without restart or stopping ordinary metrics')
        def switch_proxy(enabled,expected):
            query=urllib.request.Request(base_url+f'/api/v1/nodes/{node_id}/proxy-collection',
                data=json.dumps({'enabled':enabled,'expected_enabled':expected}).encode(),
                headers={'Content-Type':'application/json','Origin':base_url})
            with opener.open(query,timeout=3) as response:require(json.load(response)['enabled'] is enabled,'proxy switch saved over HTTPS')
        def wait_for_proxy_switch(enabled):
            deadline=time.monotonic()+35
            while time.monotonic()<deadline:
                observed=next(n for n in listing() if n['id']==node_id)
                if observed['metrics'].get('collection_policy',{}).get('proxy_enabled') is enabled:
                    if not enabled or any(p['pid']==proxy.pid for p in observed['metrics']['proxy_services']):return observed
                time.sleep(.2)
            raise AssertionError('Agent did not apply proxy collection switch')
        switch_proxy(False,True)
        proxy_disabled=wait_for_proxy_switch(False)
        require(proxy_disabled['metrics']['proxy_services']==[] and proxy.poll() is None and agent.pid==pid_before
                and proxy_disabled['metrics']['ai_tools'] and proxy_disabled['metrics']['collection_policy']['users_enabled'] is False
                and proxy_disabled['metrics']['collection_policy']['permissions_enabled'] is False,
                'proxy switch stops observations but not the actual proxy, other tools, metrics or Agent process')
        graceful_shutdown()
        first_identity = persisted_identity()
        require(first_identity.get("node_id") == node_id and bool(first_identity.get("agent_token"))
                and state.stat().st_mode & 0o077 == 0, "Agent identity is persisted in private temporary SQLite state")
        started = datetime.now(timezone.utc)
        agent = subprocess.Popen([str(binary.resolve())], cwd=agent_root, env=env, stdout=agent_log, stderr=subprocess.STDOUT)
        restarted = wait_for_heartbeat(started)
        require(restarted['metrics']['network_activity']['process_id']==agent.pid
                and restarted['metrics']['network_activity']['started_at']>activity['started_at'],
                'Agent process restart begins a new network counter scope instead of inventing continuous totals')
        require(restarted['metrics']['collection_policy']['users_enabled'] is False and restarted['metrics']['users']==[],
                'disabled user collection persists before the first sample after an Agent restart')
        require(restarted['metrics']['collection_policy']['permissions_enabled'] is False and restarted['metrics']['permission_audit'] is None,
                'disabled permission collection persists before the first check after an Agent restart')
        require(restarted['metrics']['collection_policy']['proxy_enabled'] is False and restarted['metrics']['proxy_services']==[] and proxy.poll() is None,
                'disabled proxy collection persists before startup scanning while the actual proxy continues running')
        require(restarted["metrics"]["hardware_details"] == hardware,
                "daily hardware cache survives Agent restart without changing the sample timestamp")
        require(restarted['metrics']['host_security']==metrics['host_security'],
                'hourly security cache survives Agent restart without rescanning or changing its sample timestamp')
        require({node["id"] for node in listing()} == before | {node_id}, "restarting the Agent never creates a duplicate node")
        switch_proxy(True,False)
        proxy_enabled=wait_for_proxy_switch(True)
        require(proxy_enabled['metrics']['inventory_collected_at']>proxy_disabled['metrics']['inventory_collected_at']
                and proxy_enabled['metrics']['collection_policy']['users_enabled'] is False
                and proxy_enabled['metrics']['collection_policy']['permissions_enabled'] is False,
                'proxy re-enable collects a fresh listener snapshot without enabling user or permission scans')
        switch_users(True,False)
        enabled=wait_for_user_switch(True)
        require(bool(enabled['metrics']['users']) and enabled['metrics']['inventory_collected_at']>disabled['metrics']['inventory_collected_at'],
                're-enabling user collection obtains a new inventory instead of replaying old status')
        require(enabled['metrics']['permission_audit'] is None,'re-enabling users does not restart permission checks')
        switch_permissions(True,False)
        resumed=wait_for_permission_switch(True)
        require(resumed['metrics']['permission_audit']['collected_at']>disabled['metrics']['permission_audit']['collected_at']
                and resumed['metrics']['users'],
                're-enabling permission collection makes a fresh audit without disabling users')
        def monitoring_request(path, payload=None):
            query=urllib.request.Request(base_url+path, data=None if payload is None else json.dumps(payload).encode(),
                headers={'Content-Type':'application/json','Origin':base_url})
            with opener.open(query,timeout=4) as response:return json.load(response)
        def managed():
            return next(m for m in monitoring_request('/api/v1/machines')['machines'] if m['node_id']==node_id)
        monitoring_path=f'/api/v1/machines/{node_id}/monitoring'
        monitoring_request(monitoring_path,{'confirmation':node_id,'paused':True,'expected_revision':0})
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            paused_machine=managed()
            if paused_machine['monitoring_applied_revision']==1:break
            time.sleep(.2)
        else:raise AssertionError('Real Agent did not acknowledge monitoring pause')
        frozen=next(n for n in listing() if n['id']==node_id)
        require(frozen['monitoring_paused'] and not frozen['online'] and agent.poll() is None
                and not any(n['node_id']==node_id for n in live_listing()),
                'paused Agent remains alive with unchanged identity but is not shown as online/live monitoring')
        with sqlite3.connect('file:'+str(state)+'?mode=ro',uri=True) as db:
            queued_before=db.execute('SELECT COUNT(*) FROM outbox').fetchone()[0]
        time.sleep(12)
        require(next(n for n in listing() if n['id']==node_id)['metrics']==frozen['metrics'],
                'pause stops all full metrics, slow scans and reported samples instead of inventing zeros')
        graceful_shutdown()
        identity_before_pause_restart=persisted_identity()
        agent=subprocess.Popen([str(binary.resolve())],cwd=agent_root,env=env,stdout=agent_log,stderr=subprocess.STDOUT)
        time.sleep(8)
        with sqlite3.connect('file:'+str(state)+'?mode=ro',uri=True) as db:
            queued_after=db.execute('SELECT COUNT(*) FROM outbox').fetchone()[0]
        require(agent.poll() is None and next(n for n in listing() if n['id']==node_id)['metrics']==frozen['metrics']
                and queued_after==queued_before and persisted_identity()==identity_before_pause_restart,
                'persisted pause survives Agent restart before collector initialization, without new samples or outbox growth')
        control_before=managed()['control_seen_at']
        before_control_requests=next(g for g in monitoring_request('/api/v1/platform-network')['server']['groups'] if g['group']=='http_agent')['started']
        deadline=time.monotonic()+65
        while time.monotonic()<deadline:
            if managed()['control_seen_at']!=control_before:break
            time.sleep(.5)
        else:raise AssertionError('Paused Agent did not make its minute control poll')
        after_control_requests=next(g for g in monitoring_request('/api/v1/platform-network')['server']['groups'] if g['group']=='http_agent')['started']
        still_paused=next(n for n in listing() if n['id']==node_id)
        require(still_paused['metrics']==frozen['metrics'] and still_paused['last_seen']==frozen['last_seen']
                and after_control_requests-before_control_requests==1,
                'minute lightweight control does not refresh last_seen, metrics or history')
        started=datetime.now(timezone.utc)
        monitoring_request(monitoring_path,{'confirmation':node_id,'paused':False,'expected_revision':1})
        deadline=time.monotonic()+70
        while time.monotonic()<deadline:
            observed=next(n for n in listing() if n['id']==node_id)
            if observed['online'] and observed['metrics'].get('agent_control')=={'revision':2,'paused':False}:break
            time.sleep(.5)
        else:raise AssertionError('Agent did not resume at the next minute control poll')
        require(parse_timestamp(observed['metrics']['collected_at'])>=started and persisted_identity()==identity_before_pause_restart
                and observed['metrics']['privilege']==frozen['metrics']['privilege'] and agent.poll() is None,
                'GUI resume obtains fresh real metrics using the same running Agent, identity, credential and privileges')
        graceful_shutdown()
        require(persisted_identity() == first_identity, "restarted Agent reuses the same node identity and credential")
    except Exception:
        print_isolated_log_tail(agent_log, "Agent", [enrollment_token])
        raise
    finally:
        stop_process(agent)
        stop_process(proxy)
        agent_log.close()


def smoke(args):
    ensure_sandbox(args)
    test_parse_timestamp_nanoseconds()
    print("PASS RFC3339 nanoseconds and offsets parse correctly on Python 3.10", flush=True)
    postgres = server = None
    with tempfile.TemporaryDirectory(prefix="easymanage-smoke.", dir="/tmp") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        env = {"PATH": "/usr/bin:/bin:/usr/lib/postgresql/18/bin", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8", "TMPDIR": str(root)}
        with (root / "processes.log").open("w+") as log:
            try:
                data, sockets = root / "postgres", root / "sockets"
                sockets.mkdir(mode=0o700)
                pg_bin = Path("/usr/lib/postgresql/18/bin")
                run_checked(str(pg_bin / "initdb"), "-D", str(data), "-U", "smoke", "--auth=trust", "--encoding=UTF8", "--no-locale", env=env, log=log)
                postgres = subprocess.Popen([str(pg_bin / "postgres"), "-D", str(data), "-h", "127.0.0.1", "-p", "55432", "-k", str(sockets),
                                             "-c", "shared_buffers=8MB", "-c", "max_connections=12", "-c", "fsync=off", "-c", "synchronous_commit=off"],
                                            env=env, stdout=log, stderr=subprocess.STDOUT)
                deadline = time.monotonic() + 15
                while subprocess.run([str(pg_bin / "pg_isready"), "-h", "127.0.0.1", "-p", "55432", "-U", "smoke"], env=env,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=2).returncode:
                    if postgres.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("Temporary PostgreSQL did not start")
                    time.sleep(0.1)
                run_checked(str(pg_bin / "createdb"), "-h", "127.0.0.1", "-p", "55432", "-U", "smoke", "easymanage_smoke", env=env, log=log)
                ca_certificate, certificate, key = create_test_certificates(root, env, log)
                web = root / "web"
                web.mkdir(mode=0o700)
                (web / "index.html").write_text("<!doctype html><html><body>isolated-smoke-dashboard</body></html>")
                (web / "guide/assets").mkdir(parents=True, mode=0o700)
                (web / "guide/index.html").write_text("<!doctype html><html><body>isolated-smoke-vitepress</body></html>")
                (web / "guide/easymanage.html").write_text("<!doctype html><html><body>isolated-smoke-platform-docs</body></html>")
                (web / "guide/assets/app.js").write_text("console.log('isolated-smoke-vitepress')")
                empty_env = root / "server.env"
                (root / "storage-guard").symlink_to(root, target_is_directory=True)
                empty_env.write_text("")
                known_hosts = root / "known_hosts"
                known_hosts.write_text("")
                known_hosts.chmod(0o600)
                node_id, enrollment = str(uuid.uuid4()), secrets.token_hex(32)
                base_url = "https://127.0.0.1:58443"
                registry = root / "targets.json"
                registry.write_text(json.dumps({"targets": [{"node_id": node_id, "hostname": "isolated-smoke-node", "host": "198.51.100.1", "port": 22,
                    "default_user": "smoke", "agent_home": "/home/smoke/.easymanage", "known_hosts": str(known_hosts),
                    "agent_sha256": "a" * 64, "server_url": base_url, "ca_sha256": hashlib.sha256(ca_certificate.read_bytes()).hexdigest()}]}))
                registry.chmod(0o600)
                server_env = env | {"EASYMANAGE_ENV_FILE": str(empty_env), "EASYMANAGE_DISK_SCAN_STORAGE_PATH": str(root / "storage-guard"), "DATABASE_URL": "postgresql://smoke@127.0.0.1:55432/easymanage_smoke",
                    "EASYMANAGE_ENROLLMENT_TOKEN": enrollment, "EASYMANAGE_BIND": "127.0.0.1:58443", "EASYMANAGE_DATABASE_MAX_CONNECTIONS": "2",
                    "EASYMANAGE_TLS_CERTIFICATE": str(certificate), "EASYMANAGE_TLS_PRIVATE_KEY": str(key), "EASYMANAGE_WEB_ROOT": str(web),
                    "EASYMANAGE_OPERATIONS_TARGETS": str(registry), "RUST_LOG": "warn", "TOKIO_WORKER_THREADS": "2"}
                if args.machine_onboarding:
                    if args.agent_binary is None:
                        raise RuntimeError("Machine onboarding tests require --agent-binary")
                    controller_home = root / "controller-home"
                    candidate = controller_home / ".easymanage/releases/current/target/release/easymanage-agent"
                    candidate.parent.mkdir(parents=True, mode=0o700)
                    shutil.copyfile(args.agent_binary, candidate)
                    candidate.chmod(0o700)
                    shim = root / "ssh-loopback-fixture.so"
                    run_checked("/usr/bin/cc", "-shared", "-fPIC", "-o", str(shim),
                                str(Path(__file__).parent / "fixtures/ssh_loopback_redirect.c"), "-ldl", env=env, log=log)
                    server_env |= {"HOME": str(controller_home), "USER": "smoke", "LD_PRELOAD": str(shim),
                                   "EASYMANAGE_AGENT_CA_CERTIFICATE": str(ca_certificate),"EASYMANAGE_DATABASE_MAX_CONNECTIONS":"1"}
                if args.previous_server_binary is not None:
                    from resource_upgrade_smoke import exercise as exercise_upgrade
                    exercise_upgrade(args.previous_server_binary,args.server_binary,root,server_env,base_url,
                        ssl.create_default_context(cafile=str(ca_certificate)),enrollment,log,require)
                server = subprocess.Popen([str(args.server_binary.resolve())], cwd=root, env=server_env, stdout=log, stderr=subprocess.STDOUT)
                context = ssl.create_default_context(cafile=str(ca_certificate))
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
                deadline = time.monotonic() + 20
                while True:
                    try:
                        with opener.open(base_url + "/api/v1/health/ready", timeout=1) as response:
                            if response.status == 200:
                                break
                    except (OSError, urllib.error.URLError):
                        pass
                    if server.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("Isolated EasyManage server did not become ready")
                    time.sleep(0.1)
                exercise_api(base_url, context, enrollment, node_id)
                from documents_smoke import exercise as exercise_documents
                saved_document = exercise_documents(base_url, context, require)
                from external_platforms_smoke import exercise as exercise_external_platforms
                saved_external_platforms = exercise_external_platforms(base_url, context, require)
                if args.usage_history:
                    from usage_history_smoke import exercise as exercise_usage_history
                    exercise_usage_history(root, base_url, context, enrollment, require)
                if args.disk_space:
                    from disk_space_api_smoke import exercise as exercise_disk_space
                    exercise_disk_space(root, base_url, context, enrollment, require)
                if args.agent_binary is not None and not args.machine_onboarding:
                    exercise_agent(args.agent_binary, root, env, base_url, ca_certificate, enrollment)
                if args.machine_onboarding:
                    from machine_onboarding_smoke import exercise
                    def restart_controller():
                        nonlocal server
                        stop_process(server)
                        server = subprocess.Popen([str(args.server_binary.resolve())], cwd=root, env=server_env, stdout=log, stderr=subprocess.STDOUT)
                        deadline=time.monotonic()+25
                        while time.monotonic()<deadline:
                            try:
                                with opener.open(base_url+'/api/v1/health/ready',timeout=2) as response:
                                    if response.status==200:return
                            except OSError:time.sleep(0.1)
                        raise RuntimeError('Isolated controller restart failed')
                    exercise(args.agent_binary, root, env, base_url, context, enrollment, require, restart_controller, args.machine_redeployment, args.machine_admin_credentials, args.machine_disk_space, args.machine_update,args.machine_agent_stop)
                with opener.open(base_url + "/api/v1/operations/targets", timeout=2) as response:
                    expected_target_count = len(json.load(response)["targets"])
                with opener.open(base_url+'/api/v1/machines',timeout=5) as response:
                    stopped_agent_ids=[m['node_id'] for m in json.load(response)['machines'] if m.get('agent_service_state')=='stopped']
                # Reproduce PostgreSQL outage and restart the Server with DB down.
                stop_process(postgres)
                try:
                    opener.open(base_url + "/api/v1/machines", timeout=5)
                except urllib.error.HTTPError as error:
                    body = json.load(error)
                    require(error.code == 503 and 'PostgreSQL' in body['message']
                            and '运维连接过多' not in body['message'],
                            'database outage is reported as unavailable storage rather than an SSH rate limit')
                else:
                    raise AssertionError("machine listing incorrectly accepted an unavailable database")
                for stopped_id in stopped_agent_ids:
                    req=urllib.request.Request(base_url+f'/api/v1/nodes/{stopped_id}/agent/force-restart',data=b'{}',headers={'Content-Type':'application/json','Origin':base_url})
                    try:opener.open(req,timeout=2)
                    except urllib.error.HTTPError as error:require(error.code==409,'database-independent emergency recovery cannot restart a deliberately stopped Agent')
                    else:raise AssertionError('Stopped Agent was offered an emergency restart')
                stop_process(server)
                server = subprocess.Popen([str(args.server_binary.resolve())], cwd=root, env=server_env, stdout=log, stderr=subprocess.STDOUT)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    try:
                        with opener.open(base_url + "/api/v1/operations/recovery", timeout=2) as response:
                            require(not json.load(response)["database_healthy"], "Server restart while PostgreSQL is down keeps recovery reachable")
                            break
                    except OSError:
                        time.sleep(0.1)
                else:
                    raise RuntimeError("Recovery API was unavailable during database outage")
                with opener.open(base_url + "/", timeout=2) as response:
                    require("isolated-smoke-dashboard" in response.read().decode(), "dashboard remains reachable during DB outage")
                with opener.open(base_url+'/api/v1/platform-network',timeout=5) as response:
                    offline_network=json.load(response)
                    require(not offline_network['agents_available'] and offline_network['server']['process_id']==server.pid,
                            'local platform network counters remain available during database outage without fake Agent zeros')
                with opener.open(base_url + "/api/v1/operations/targets", timeout=2) as response:
                    require(len(json.load(response)["targets"]) == expected_target_count, "trusted targets remain available without PostgreSQL")
                try:
                    opener.open(base_url + "/api/v1/nodes", timeout=2)
                except urllib.error.HTTPError as error:
                    require(error.code == 503, "monitoring fails fast with 503 until database initialization completes")
                else:
                    raise AssertionError("uninitialized monitoring was not gated")
                try:
                    opener.open(base_url + "/api/v1/machines", timeout=5)
                except urllib.error.HTTPError as error:
                    body = json.load(error)
                    require(error.code == 503 and '数据库' in body['message']
                            and '运维连接过多' not in body['message'],
                            'cold-start machine management correctly reports that database initialization is pending')
                else:
                    raise AssertionError("machine listing incorrectly accepted an unavailable database")
                for endpoint in (f"/api/v1/nodes/{node_id}/agent/force-restart", "/api/v1/platform/force-restart"):
                    req = urllib.request.Request(base_url + endpoint, data=json.dumps({"username":"smoke","password":"fixture"}).encode(),
                        headers={"Content-Type":"application/json","Origin":"https://cross-origin.invalid"})
                    try:
                        opener.open(req, timeout=2)
                    except urllib.error.HTTPError as error:
                        require(error.code == 403, "force restart still enforces same origin with DB down")
                    else:
                        raise AssertionError("unauthorized restart accepted")
                postgres = subprocess.Popen([str(pg_bin / "postgres"), "-D", str(data), "-h", "127.0.0.1", "-p", "55432", "-k", str(sockets),
                    "-c", "shared_buffers=8MB", "-c", "max_connections=12", "-c", "fsync=off", "-c", "synchronous_commit=off"], env=env, stdout=log, stderr=subprocess.STDOUT)
                deadline = time.monotonic() + 25
                while time.monotonic() < deadline:
                    try:
                        with opener.open(base_url + "/api/v1/health/ready", timeout=2) as response:
                            if response.status == 200: break
                    except OSError:
                        time.sleep(0.3)
                else: raise RuntimeError("Server did not recover automatically after PostgreSQL returned")
                require(True, "database recovery automatically restores monitoring without another platform restart")
                with opener.open(base_url + '/api/v1/machines', timeout=5) as response:
                    require(response.status == 200 and json.load(response)['machines'],
                            'machine management recovers automatically after PostgreSQL returns')
                with opener.open(base_url+'/api/v1/documents/gpu-comparison',timeout=5) as response:
                    require(json.load(response)==saved_document, 'saved Markdown persists across real platform and PostgreSQL restarts')
                with opener.open(base_url+'/api/v1/external-platforms',timeout=5) as response:
                    require({item['id']:item for item in json.load(response)['platforms']} == saved_external_platforms,
                            'active and archived external shortcuts persist across real platform and PostgreSQL restarts')
                with opener.open(base_url + "/api/v1/collection-settings", timeout=3) as response:
                    before_restart = json.load(response)["policy"]
                stop_process(server)
                server = subprocess.Popen([str(args.server_binary.resolve())], cwd=root, env=server_env, stdout=log, stderr=subprocess.STDOUT)
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    try:
                        with opener.open(base_url + "/api/v1/collection-settings", timeout=1) as response:
                            restored_settings = json.load(response)
                            after_restart = restored_settings["policy"]
                        break
                    except (OSError, urllib.error.URLError):
                        time.sleep(0.1)
                else:
                    raise AssertionError("Restarted isolated platform did not become ready")
                require(after_restart == before_restart, "collection settings survive a real isolated platform restart")
                require(next(a for a in restored_settings['agents'] if a['node_id']==node_id)['users_enabled'] is False,
                        'per-node user collection switches survive database recovery and a platform restart')
                require(next(a for a in restored_settings['agents'] if a['node_id']==node_id)['permissions_enabled'] is False,
                        'per-node permission switches survive database recovery and a platform restart')
                require(next(a for a in restored_settings['agents'] if a['node_id']==node_id)['proxy_enabled'] is False,
                        'per-node proxy switches survive database recovery and platform restart')
                with opener.open(base_url+'/api/v1/machines',timeout=3) as response:
                    persisted_pause=next(m for m in json.load(response)['machines'] if m['node_id']==node_id)
                require(persisted_pause['monitoring_paused'] and persisted_pause['monitoring_revision']==1
                        and persisted_pause['monitoring_applied_revision']==1,
                        'monitoring pause and its acknowledgement survive database recovery and platform restart')
                print("PASS isolated integration smoke completed; synthetic fixtures and optional private-namespace collection only", flush=True)
            except Exception:
                log.flush()
                log.seek(0)
                # Fixture credentials are ephemeral; nevertheless do not print service
                # output on failures, to preserve the production no-secret-log rule.
                print("FAIL isolated smoke; process log retained only until private namespace cleanup", flush=True)
                raise
            finally:
                stop_process(server)
                stop_process(postgres)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-binary", type=Path, required=True)
    parser.add_argument("--previous-server-binary",type=Path,help="Optional schema-16 binary to test private forward migration and emergency rollback")
    parser.add_argument("--agent-binary", type=Path)
    parser.add_argument("--machine-onboarding", action="store_true", help="Opt-in SSH/SCP fixture with mocked systemd and a real installed Agent; temporary Paramiko venv required")
    parser.add_argument("--machine-update",action="store_true",help="Focused real SSH update and nonce/hash receipt while remote output stays open")
    parser.add_argument("--machine-agent-stop",action="store_true",help="Real Basic pause/heartbeat, process stop, no automatic reconnect and redeployment-only recovery")
    parser.add_argument("--machine-redeployment",action="store_true",help="Add real same-UUID uninstalled Agent redeployment coverage; skips the separate three-minute retry-exhaustion scenario")
    parser.add_argument("--machine-admin-credentials",action="store_true",help="Focused real SSH administrator password/key vault and saved-credential operations regression")
    parser.add_argument("--usage-history",action="store_true",help="Standalone seven-day history, cache, calendar boundaries and reader isolation")
    parser.add_argument("--disk-space",action="store_true",help="Read-only disk scanner task, permission and database API regression")
    parser.add_argument("--machine-disk-space",action="store_true",help="Focused disk scanner over real persistent SSH with a real Basic Agent")
    parser.add_argument("--host-netns", required=True)
    parser.add_argument("--host-mntns", required=True)
    args=parser.parse_args()
    if args.machine_redeployment or args.machine_admin_credentials or args.machine_disk_space or args.machine_update or args.machine_agent_stop:args.machine_onboarding=True
    smoke(args)
