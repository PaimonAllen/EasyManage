#!/usr/bin/env python3
"""Fixed SSH-side network switch. Secrets arrive only on stdin; no identity reset."""
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import sqlite3
import stat
import subprocess
import sys
import tempfile
import uuid
from urllib.parse import quote


class SwitchError(Exception):
    def __init__(self, code):
        self.code = code


def require(condition, code):
    if not condition:
        raise SwitchError(code)


def env_value(raw):
    raw = raw.strip()
    if raw.startswith(("'", '"')):
        try:
            parts = shlex.split(raw, comments=True)
        except ValueError:
            raise SwitchError(25) from None
        require(len(parts) == 1, 25)
        return parts[0]
    return raw.split(" #", 1)[0].strip()


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
    state_db = base / "state/state.db"
    if mode == "full":
        # A fixed, already authorized administrator operation; never execute
        # user-writable helpers as root, and preserve the existing Full service.
        result = subprocess.run(["/usr/bin/systemctl", "show", "easymanage-agent.service", "-p", "User", "--value"],
                                capture_output=True, timeout=5, check=True)
        require(result.stdout.strip() in (b"root", b""), 24)
        # systemd's empty User= means root; require an actual loaded unit too.
        loaded = subprocess.run(["/usr/bin/systemctl", "show", "easymanage-agent.service", "-p", "LoadState", "--value"],
                                capture_output=True, timeout=5, check=True)
        require(loaded.stdout.strip() == b"loaded", 24)
        # Never write through a user-owned parent as root. First-install Full
        # nodes need no Basic shadow, and root enrollment settings stay private.
        paths = [Path("/etc/easymanage/agent.env")]
        ca = Path("/etc/easymanage/tls/ca.crt")
        state_db = Path("/var/lib/easymanage-agent/state.db")
    for path in [ca, *paths]:
        require(path.exists() or path.is_symlink(), 20)
        for item in [path, *path.parents]:
            require(not item.is_symlink(), 21)
            if mode == "full" and item != Path("/"):
                info = item.stat()
                require(info.st_uid == 0 and not info.st_mode & 0o022, 21)
        info = path.stat()
        require(stat.S_ISREG(info.st_mode) and info.st_size <= 1024 * 1024, 21)
        if mode == "basic":
            require(info.st_uid == os.getuid(), 21)
        if path in paths:
            require(not info.st_mode & 0o077, 21)
    require(hashlib.sha256(ca.read_bytes()).hexdigest() == ca_hash, 22)
    prepared = []
    for path in paths:
        values = {}
        original = path.read_text().splitlines()
        for line in original:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            line = re.sub(r"^export\s+", "", line, count=1)
            key, sep, value = line.partition("=")
            key = key.strip()
            require(sep and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key) and key not in values, 25)
            values[key] = value
        identity = env_value(values.get("EASYMANAGE_NODE_ID", ""))
        if not identity:
            # Legacy deployments can keep the UUID only in SQLite. Read it
            # without creating a DB, updating metadata or rotating Agent tokens.
            require(state_db.is_file() and all(not p.is_symlink() for p in [state_db, *state_db.parents]), 23)
            try:
                with sqlite3.connect("file:" + quote(str(state_db), safe="/") + "?mode=ro", uri=True, timeout=3) as db:
                    row = db.execute("SELECT value FROM metadata WHERE key='node_id'").fetchone()
            except sqlite3.Error:
                raise SwitchError(23) from None
            require(row is not None, 23)
            identity = row[0]
        try:
            require(str(uuid.UUID(identity)) == node, 23)
        except ValueError:
            raise SwitchError(23) from None
        replacements = {"EASYMANAGE_SERVER_URL": url, "EASYMANAGE_NODE_ID": node}
        if token is not None:
            replacements["EASYMANAGE_ENROLLMENT_TOKEN"] = token
        updated = []
        for line in original:
            match = re.match(r"\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=", line)
            key = match.group(1) if match else None
            updated.append(f"{key}={replacements.pop(key)}" if key in replacements else line)
        updated.extend(f"{key}={value}" for key, value in replacements.items())
        prepared.append((path, "\n".join(updated) + "\n"))
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
    except SwitchError as error:
        # Stable allowlisted exit codes, never raw exceptions or environment data.
        print("Network switch validation failed; no identity was reset.", file=sys.stderr)
        raise SystemExit(error.code)
    except OSError:
        print("Network switch file operation failed; private recovery files may be retained.", file=sys.stderr)
        raise SystemExit(26)
    except Exception:
        print("Network switch failed; private recovery files may be retained. No identity was reset.", file=sys.stderr)
        raise SystemExit(1)
