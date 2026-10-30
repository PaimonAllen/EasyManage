#!/usr/bin/env python3
"""Run only in private namespaces: no real remote mounts or connections."""
import fcntl
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
from unittest.mock import patch

assert len(sys.argv) == 3
for kind, original in zip(("net", "mnt"), sys.argv[1:]):
    assert os.readlink("/proc/self/ns/" + kind) != original
module = runpy.run_path("agent/src/platform/linux/network_capacity_probe.py")
run = module["run"]
with tempfile.TemporaryDirectory(dir="/tmp") as directory:
    root = Path(directory)
    request = {"lock": str(root / "capacity.lock"), "path": "/tmp"}
    before = module["identity"]("/tmp")
    request.update(zip(("device", "root", "filesystem", "source"), before[1:]))
    assert request["filesystem"] == "tmpfs"
    result = run(request)
    assert result["status"] == "ok" and result["total_bytes"] > 0 and result["collected_at"]
    assert 0 <= result["available_bytes"] <= result["total_bytes"]
    assert (root / "capacity.lock").stat().st_mode & 0o777 == 0o600
    # A lock held by an earlier process prevents touching the path at all,
    # including after a parent restart. All stdout remains bounded and sanitized.
    with (root / "capacity.lock").open("rb") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with patch("os.statvfs", side_effect=AssertionError("remote lookup forbidden")):
            assert run(request) == {"status": "busy"}
        child = subprocess.run(["/usr/bin/python3", "-I", "-S", "agent/src/platform/linux/network_capacity_probe.py", json.dumps(request)],
                               capture_output=True, timeout=5)
        assert child.returncode == 0 and json.loads(child.stdout) == {"status": "busy"}
        assert not child.stderr and len(child.stdout) < 1024
    with patch("os.statvfs", side_effect=AssertionError("wrong mount must not be probed")):
        assert run(request | {"device": "0:9999"}) == {"status": "changed"}
    # A mount replaced during statvfs must not publish a wrong filesystem's data.
    real_identity = run.__globals__["identity"]
    values = iter([before, None])
    run.__globals__["identity"] = lambda _: next(values)
    assert run(request) == {"status": "changed"}
    run.__globals__["identity"] = real_identity
    print("PASS isolated capacity, timestamp, persistent cross-process lock, bounded child output and mount replacement rejection")
