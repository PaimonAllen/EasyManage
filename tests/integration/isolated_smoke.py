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
        "temperatures": {"collected_at": (collected - timedelta(seconds=20)).isoformat(),
                         "cpu_celsius": 51.5, "ssds": [{"device": "nvme0", "temperature_celsius": 42.0}]},
        "load_average_one": 0.4, "load_average_five": 0.3, "load_average_fifteen": 0.2,
        "memory_total_bytes": 1073741824, "memory_used_bytes": 536870912,
        "swap_total_bytes": 0, "swap_used_bytes": 0, "uptime_seconds": 120,
        "disks": [
            {"name": "synthetic-disk", "mount_point": "/", "file_system": "ext4", "total_bytes": 1000000, "available_bytes": 600000, "read_bytes_per_second": 1536.0, "written_bytes_per_second": 768.0},
            {"name": "synthetic-disk", "mount_point": "/data", "file_system": "ext4", "total_bytes": 1000000, "available_bytes": 600000, "read_bytes_per_second": 1536.0, "written_bytes_per_second": 768.0},
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
    invalid_temperature = json.loads(json.dumps(report))
    invalid_temperature["metrics"]["temperatures"]["cpu_celsius"] = 999
    require(request("/api/v1/agent/report", invalid_temperature, {"x-easymanage-agent-token": agent_token})[0] == 400,
            "invalid sensor temperature rejected before database writes")
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
            and parse_timestamp(node["metrics"]["collected_at"]) == collected,
            "delayed report never overwrites newer latest metrics")
    _, history = request(f"/api/v1/nodes/{node_id}/history")
    require(len(history["points"]) == 2 and history["points"][0]["cpu_usage_percent"] == 1.0
            and history["points"][0]["disk_collected_at"] is None
            and history["points"][1]["cpu_usage_percent"] == 37.5,
            "history remains chronological when delayed samples arrive")
    require(node["hostname"] == inventory["hostname"] and node["os_version"] == inventory["os_version"]
            and node["agent_version"] == "0.1.0", "delayed report never rolls back node inventory or Agent version")
    status, performance = request("/api/v1/platform-performance")
    agent_history = next(item for item in performance["agents"] if item["node_id"] == node_id)["stats"]
    agent_storage = next(item for item in performance["agents"] if item["node_id"] == node_id)["storage"]
    require(status == 200 and performance["period_seconds"] == 86400
            and performance["server"]["stats"]["sample_count"] >= 1
            and agent_history["sample_count"] == 2
            and agent_history["cpu_percent_of_host"]["samples"] == 2
            and abs(agent_history["cpu_percent_of_host"]["average"] - 0.5) < 0.000001
            and agent_history["cpu_percent_of_host"]["maximum"] == 0.75
            and agent_history["resident_memory_bytes"]["maximum"] == 2097152.0,
            "24-hour process statistics include controller and all real Agent report samples")
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


def exercise_agent(binary, root, environment, base_url, ca_certificate, enrollment_token):
    """Exercise the real collector only inside the guarded private namespaces."""
    agent = None
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
        started = datetime.now(timezone.utc)
        agent = subprocess.Popen([str(binary.resolve())], cwd=agent_root, env=env, stdout=agent_log, stderr=subprocess.STDOUT)
        node = wait_for_heartbeat(started)
        metrics = node["metrics"]
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
                and set(metrics["host_security"]) == {"apparmor_enabled", "aslr_mode", "ptrace_scope", "kernel_lockdown"},
                "real Agent reports bounded host security observations without inferring unknowns")
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
            if applied and applied["metrics"].get("collection_policy") == expected_policy:
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
        graceful_shutdown()
        first_identity = persisted_identity()
        require(first_identity.get("node_id") == node_id and bool(first_identity.get("agent_token"))
                and state.stat().st_mode & 0o077 == 0, "Agent identity is persisted in private temporary SQLite state")
        started = datetime.now(timezone.utc)
        agent = subprocess.Popen([str(binary.resolve())], cwd=agent_root, env=env, stdout=agent_log, stderr=subprocess.STDOUT)
        restarted = wait_for_heartbeat(started)
        require(restarted["metrics"]["hardware_details"] == hardware,
                "daily hardware cache survives Agent restart without changing the sample timestamp")
        require({node["id"] for node in listing()} == before | {node_id}, "restarting the Agent never creates a duplicate node")
        graceful_shutdown()
        require(persisted_identity() == first_identity, "restarted Agent reuses the same node identity and credential")
    except Exception:
        print_isolated_log_tail(agent_log, "Agent", [enrollment_token])
        raise
    finally:
        stop_process(agent)
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
                server_env = env | {"EASYMANAGE_ENV_FILE": str(empty_env), "DATABASE_URL": "postgresql://smoke@127.0.0.1:55432/easymanage_smoke",
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
                    exercise(args.agent_binary, root, env, base_url, context, enrollment, require, restart_controller, args.machine_redeployment)
                with opener.open(base_url + "/api/v1/operations/targets", timeout=2) as response:
                    expected_target_count = len(json.load(response)["targets"])
                # Reproduce PostgreSQL outage and restart the Server with DB down.
                stop_process(postgres)
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
                with opener.open(base_url + "/api/v1/operations/targets", timeout=2) as response:
                    require(len(json.load(response)["targets"]) == expected_target_count, "trusted targets remain available without PostgreSQL")
                try:
                    opener.open(base_url + "/api/v1/nodes", timeout=2)
                except urllib.error.HTTPError as error:
                    require(error.code == 503, "monitoring fails fast with 503 until database initialization completes")
                else:
                    raise AssertionError("uninitialized monitoring was not gated")
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
                with opener.open(base_url + "/api/v1/collection-settings", timeout=3) as response:
                    before_restart = json.load(response)["policy"]
                stop_process(server)
                server = subprocess.Popen([str(args.server_binary.resolve())], cwd=root, env=server_env, stdout=log, stderr=subprocess.STDOUT)
                deadline = time.monotonic() + 20
                while time.monotonic() < deadline:
                    try:
                        with opener.open(base_url + "/api/v1/collection-settings", timeout=1) as response:
                            after_restart = json.load(response)["policy"]
                        break
                    except (OSError, urllib.error.URLError):
                        time.sleep(0.1)
                else:
                    raise AssertionError("Restarted isolated platform did not become ready")
                require(after_restart == before_restart, "collection settings survive a real isolated platform restart")
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
    parser.add_argument("--agent-binary", type=Path)
    parser.add_argument("--machine-onboarding", action="store_true", help="Opt-in SSH/SCP fixture with mocked systemd and a real installed Agent; temporary Paramiko venv required")
    parser.add_argument("--machine-redeployment",action="store_true",help="Add real same-UUID uninstalled Agent redeployment coverage; skips the separate three-minute retry-exhaustion scenario")
    parser.add_argument("--host-netns", required=True)
    parser.add_argument("--host-mntns", required=True)
    args=parser.parse_args()
    if args.machine_redeployment:args.machine_onboarding=True
    smoke(args)
