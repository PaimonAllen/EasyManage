#!/usr/bin/env python3
"""Read-only, controller-owned admission check for same-identity Agent repair."""
import json
import os
from pathlib import Path
import shlex
import shutil
import sqlite3
import stat
import subprocess
import sys
import uuid
from urllib.parse import quote


def check(home, node, mode):
    expected = uuid.UUID(node)
    base = Path(home)
    if mode not in ("basic", "full") or (mode == "full") != (os.geteuid() == 0):
        raise ValueError()
    if base != Path.home() / ".easymanage" and mode == "basic":
        raise ValueError()
    if mode == "basic":
        # Never turn a Full/system installation into an ordinary user service.
        for path in [Path("/etc/easymanage/agent.env"), Path("/var/lib/easymanage-agent"),
                     Path("/usr/local/sbin/easymanage-agent"), Path("/etc/systemd/system/easymanage-agent.service")]:
            if path.exists() or path.is_symlink():
                raise ValueError()
        if shutil.which("systemctl"):
            loaded = subprocess.run(["systemctl", "show", "easymanage-agent.service", "-p", "LoadState", "--value"],
                                    capture_output=True, timeout=5)
            if loaded.stdout.strip() not in (b"", b"not-found"):
                raise ValueError()
        deployments = [(base / "config/agent.env", base / "state/state.db")]
    else:
        deployments = [(Path("/etc/easymanage/agent.env"), Path("/var/lib/easymanage-agent/state.db"))]
    for env, database in deployments:
        for path in [env, database, Path(str(database) + "-wal"), Path(str(database) + "-shm")]:
            for item in [path, *path.parents]:
                if item.is_symlink():
                    raise ValueError()
            if path.exists():
                info = path.stat()
                if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_nlink != 1 or info.st_mode & 0o022:
                    raise ValueError()
        stored_id = None
        if database.exists():
            with sqlite3.connect("file:" + quote(str(database), safe="/") + "?mode=ro", uri=True, timeout=3) as db:
                row = db.execute("SELECT value FROM metadata WHERE key='node_id'").fetchone()
            if not row or uuid.UUID(row[0]) != expected:
                raise ValueError()
            stored_id = row[0]
        if env.exists():
            if env.stat().st_size > 65536:
                raise ValueError()
            values = {}
            for line in env.read_text().splitlines():
                words = shlex.split(line, comments=True)
                if words and words[0] == "export":
                    words = words[1:]
                if not words:
                    continue
                if len(words) != 1 or "=" not in words[0]:
                    raise ValueError()
                key, value = words[0].split("=", 1)
                if key in values:
                    raise ValueError()
                values[key] = value
            declared = values.get("EASYMANAGE_NODE_ID", "")
            if declared and uuid.UUID(declared) != expected:
                raise ValueError()
            if not declared and stored_id is None:
                raise ValueError()
            if values.get("EASYMANAGE_AGENT_STATE_DB", str(database)) != str(database):
                # A custom database must be migrated explicitly, never silently abandoned.
                raise ValueError()
    return {"same_identity": True}


if __name__ == "__main__":
    try:
        print(json.dumps(check(*sys.argv[1:])))
    except Exception:
        print("Existing Agent identity, permissions or database location could not be verified; no files changed.", file=sys.stderr)
        raise SystemExit(23) from None
