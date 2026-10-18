#!/usr/bin/env python3
"""Fixed SSH-side network switch. Secrets arrive only on stdin; no identity reset."""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tempfile
import uuid


def main():
    home, node, url, ca_hash, mode = sys.argv[1:]
    uuid.UUID(node)
    if not re.fullmatch(r"/(?:home/[a-zA-Z0-9_.-]+|root)/\.easymanage", home):
        raise ValueError()
    if not re.fullmatch(r"https://127\.0\.0\.1:[1-9][0-9]{0,4}", url) or int(url.rsplit(":", 1)[1]) > 65535:
        raise ValueError()
    if mode not in ("basic", "full") or (mode == "full") != (os.geteuid() == 0):
        raise ValueError()
    payload = sys.stdin.read(4097)
    if len(payload) > 4096:
        raise ValueError()
    token = json.loads(payload).get("enrollment_token")
    if token is not None and not re.fullmatch(r"[a-f0-9]{64}", token):
        raise ValueError()
    base = Path(home)
    paths = [base / "config/agent.env"]
    ca = base / "config/ca.crt"
    if mode == "full":
        # A fixed, already authorized administrator operation; never execute
        # user-writable helpers as root, and preserve the existing Full service.
        result = subprocess.run(["/usr/bin/systemctl", "show", "easymanage-agent.service", "-p", "User", "--value"],
                                capture_output=True, timeout=5, check=True)
        if result.stdout.strip() != b"root":
            raise ValueError()
        # Never write through a user-owned parent as root. First-install Full
        # nodes need no Basic shadow, and root enrollment settings stay private.
        paths = [Path("/etc/easymanage/agent.env")]
        ca = Path("/etc/easymanage/tls/ca.crt")
    for path in [ca, *paths]:
        for item in [path, *path.parents]:
            if item.is_symlink():
                raise ValueError()
            if mode == "full" and item != Path("/"):
                info = item.stat()
                if info.st_uid != 0 or info.st_mode & 0o022:
                    raise ValueError()
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > 1024 * 1024:
            raise ValueError()
        if path in paths and (info.st_mode & 0o077 or (mode == "basic" and info.st_uid != os.getuid())):
            raise ValueError()
    if hashlib.sha256(ca.read_bytes()).hexdigest() != ca_hash:
        raise ValueError()
    prepared = []
    for path in paths:
        values = {}
        for line in path.read_text().splitlines():
            if not line or line.startswith("#"):
                continue
            key, sep, value = line.partition("=")
            if not sep or key in values:
                raise ValueError()
            values[key] = value
        if str(uuid.UUID(values.get("EASYMANAGE_NODE_ID", ""))) != node:
            raise ValueError()
        values["EASYMANAGE_SERVER_URL"] = url
        if token is not None:
            values["EASYMANAGE_ENROLLMENT_TOKEN"] = token
        prepared.append((path, "".join(f"{key}={value}\n" for key, value in values.items())))
    # Validate every file before any mutation. Keep per-file private rollback
    # copies; never touch SQLite identity, Agent tokens or binary/service mode.
    for path, text in prepared:
        backup = path.with_name("agent.env.before-ssh-" + uuid.uuid4().hex)
        with backup.open("xb") as stream:
            stream.write(path.read_bytes())
        backup.chmod(0o600)
        info = path.stat()
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".ssh-network-", delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(text.encode())
        temporary.chmod(0o600)
        if os.geteuid() == 0:
            os.chown(temporary, info.st_uid, info.st_gid)
            os.chown(backup, info.st_uid, info.st_gid)
        temporary.replace(path)


if __name__ == "__main__":
    os.umask(0o077)
    try:
        main()
    except Exception:
        print("Network switch failed; private recovery files may be retained. No identity was reset.", file=sys.stderr)
        raise SystemExit(1)
