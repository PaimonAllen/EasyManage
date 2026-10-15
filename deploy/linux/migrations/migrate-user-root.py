#!/usr/bin/env python3
"""Move an existing user's deployment to ~/.easymanage, preserving identities.

Dry run by default. Stops/restarts only previously active USER services; never
starts Basic in place of Full or invokes sudo. Full system state is untouched.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import ssl
import stat
import subprocess
import tempfile
import time
import urllib.request

UNITS = ("easymanage-server.service", "easymanage-agent.service")
OLD = ".local/share/easymanage"
NEW = ".easymanage"


def control(*args, check=True):
    result = subprocess.run(["systemctl", "--user", *args], capture_output=True, text=True, timeout=35)
    if check and result.returncode:
        raise RuntimeError("User service operation failed: " + args[0])
    return result


def owned(path, directory=False):
    info = path.lstat()
    if info.st_uid != os.getuid() or not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise RuntimeError("Refusing unowned, symlink or special deployment path: " + str(path))


def atomic(path, data, mode):
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".migration-", delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(data)
    temporary.chmod(mode)
    temporary.replace(path)


def rewrite_json(value, old, new):
    if isinstance(value, str):
        return new + value[len(old):] if value == old or value.startswith(old + "/") else value
    if isinstance(value, list):
        return [rewrite_json(item, old, new) for item in value]
    if isinstance(value, dict):
        return {key: rewrite_json(item, old, new) for key, item in value.items()}
    return value


def configuration(path, old, new):
    text = path.read_text()
    if path.suffix == ".json":
        return (json.dumps(rewrite_json(json.loads(text), old, new), ensure_ascii=False, indent=2) + "\n").encode()
    # Change only absolute path values. Do not evaluate shell or alter passwords,
    # database URLs, enrollment tokens, SSH keys or certificate bytes.
    lines = []
    for line in text.splitlines(keepends=True):
        key, separator, value = line.partition("=")
        stripped = value.rstrip("\r\n")
        path_setting = key.endswith(("_FILE", "_CERTIFICATE", "_PRIVATE_KEY", "_ROOT", "_TARGETS", "_STATE_DB"))
        if separator and path_setting and (stripped == old or stripped.startswith(old + "/")):
            line = key + "=" + new + value[len(old):]
        lines.append(line)
    return "".join(lines).encode()


def ready(base):
    ca = base / "config/ca.crt"
    context = ssl.create_default_context(cafile=str(ca))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
    deadline = time.monotonic() + 50
    while time.monotonic() < deadline:
        try:
            with opener.open("https://127.0.0.1:8443/api/v1/health/ready", timeout=3) as response:
                if response.status == 200:
                    return
        except OSError:
            pass
        time.sleep(1)
    raise RuntimeError("Migrated platform readiness failed")


def migrate(home, server_binary=None, apply=False, ctl=control, health=ready):
    old, new = home / OLD, home / NEW
    owned(home, True)
    if new.exists() or new.is_symlink():
        raise RuntimeError("Destination already exists; refusing to merge or overwrite")
    owned(old, True)
    owned(old / "config", True)
    for parent in old.parents:
        if parent.is_symlink():
            raise RuntimeError("Legacy path contains a symlink")
    active = [unit for unit in UNITS if ctl("is-active", "--quiet", unit, check=False).returncode == 0]
    units = []
    for unit in UNITS:
        path = home / ".config/systemd/user" / unit
        if path.exists() or path.is_symlink():
            owned(path)
            units.append(path)
    files = [path for path in (old / "config").rglob("*") if path.suffix in (".env", ".json")]
    inventory = old / "releases/current/deploy/local/fleet.json"
    if inventory.exists():
        files.append(inventory)
    for path in files:
        owned(path)
        configuration(path, str(old), str(new))  # Validate before stopping.
    if "easymanage-server.service" in active:
        if server_binary is None:
            raise RuntimeError("Active platform requires a newly built --server-binary")
        owned(server_binary)
        with server_binary.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                raise RuntimeError("Platform candidate is not ELF")
    print("Move:", old, "->", new, flush=True)
    print("Previously active user services:", ", ".join(active) or "none; Full system Agent remains untouched", flush=True)
    if not apply:
        print("Dry run only; use --apply to migrate and restart those user services", flush=True)
        return
    backup = Path(tempfile.mkdtemp(prefix=".easymanage-migration.", dir=home))
    originals = {path: (path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) for path in [*files, *units]}
    binary = old / "bin/easymanage-server"
    if server_binary and binary.exists():
        owned(binary)
        shutil.copy2(binary, backup / "previous-server")
        (backup / "previous-server").chmod(0o700)
    for index, (path, (data, mode)) in enumerate(originals.items()):
        atomic(backup / f"file-{index}", data, 0o600)
    (backup / "manifest.json").write_text(json.dumps({"old":str(old), "new":str(new), "active":active,
        "files":[{"path":str(path),"backup":f"file-{index}","mode":mode} for index,(path,(_,mode)) in enumerate(originals.items())]}, indent=2))
    moved = False
    candidate = None
    try:
        for unit in active:
            ctl("stop", unit)
        old.rename(new)  # Same filesystem rename: no copying running SQLite files.
        moved = True
        new.chmod(0o700)
        for path in files:
            target = new / path.relative_to(old)
            atomic(target, configuration(target, str(old), str(new)), originals[path][1])
        for path in units:
            data, mode = originals[path]
            atomic(path, data.replace(str(old).encode(), str(new).encode()).replace(b"%h/" + OLD.encode(), b"%h/" + NEW.encode()), mode)
        # Old platform candidates embed old path expectations. Keep but do not
        # offer them for installation after migrating the live deployment.
        candidate = new / "platform-releases/candidate.json"
        if candidate.is_file():
            candidate.rename(backup / "previous-candidate.json")
        if server_binary:
            atomic(new / "bin/easymanage-server", server_binary.read_bytes(), 0o700)
        ctl("daemon-reload")
        for unit in active:
            ctl("start", unit)
        if "easymanage-server.service" in active:
            health(new)
        print("Migration complete. Recovery files:", backup, flush=True)
    except BaseException:
        for unit in active:
            ctl("stop", unit, check=False)
        if moved:
            for path, (data, mode) in originals.items():
                target = new / path.relative_to(old) if path.is_relative_to(old) else path
                atomic(target, data, mode)
            if (backup / "previous-server").exists():
                atomic(new / "bin/easymanage-server", (backup / "previous-server").read_bytes(), 0o700)
            if (backup / "previous-candidate.json").exists():
                (backup / "previous-candidate.json").rename(candidate)
            new.rename(old)
        ctl("daemon-reload", check=False)
        for unit in active:
            ctl("start", unit, check=False)
        print("Migration failed; attempted to restore the previous deployment. Recovery:", backup, flush=True)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--server-binary", type=Path)
    args = parser.parse_args()
    if os.geteuid() == 0:
        raise SystemExit("Run as the existing deployment owner, never root")
    os.umask(0o077)
    migrate(Path.home(), args.server_binary, args.apply)


if __name__ == "__main__":
    main()
