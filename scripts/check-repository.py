#!/usr/bin/env python3
"""Read-only repository hygiene check; prints paths/line numbers, never secret values."""
import argparse
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = (
    "README.md", "Cargo.toml", "Cargo.lock", "agent/README.md", "agent/src/app.rs",
    "agent/src/platform/contract.rs", "agent/src/platform/linux/collector.rs",
    "agent/src/platform/linux/paths.rs", "runtime/src/platform/linux/process.rs",
    "docs/architecture.md", "docs/design/Requirement.md", "deploy/docker/.gitkeep",
    "server/src/database.rs", "protocol/src/lib.rs", "runtime/src/lib.rs",
    "web/pnpm-lock.yaml", "docs/pnpm-lock.yaml", "web/src/app/App.tsx",
    "deploy/orchestration/fleet.py", "deploy/examples/fleet.example.json",
    "deploy/linux/system/easymanage-agent.service", "deploy/linux/system/easymanage-server.service",
    "deploy/linux/user/easymanage-agent.user.service", "deploy/linux/user/easymanage-server.user.service",
    "deploy/linux/bootstrap/README.md", "deploy/linux/operations/README.md",
    "deploy/linux/migrations/README.md", "deploy/windows/README.md", "deploy/macos/README.md",
    "server/config/server.env.example", "agent/config/agent.env.example",
    "tests/integration/isolated_smoke.py",
    *(f"migrations/{name}" for name in (
        "0001_monitoring_core.sql", "0002_disk_io_history.sql", "0003_platform_process_history.sql",
        "0004_permission_reviews.sql", "0005_collection_settings.sql", "0006_temperature_history.sql",
        "0007_platform_storage_footprint.sql", "0008_machine_management.sql")),
)
PRIVATE_KEY = re.compile(rb"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----")
DATABASE_CREDENTIAL = re.compile(rb"postgres(?:ql)?://[^/\s:'\"]+:([^@\s'\"\\]+)@")


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def forbidden(path):
    parts = Path(path).parts
    name = Path(path).name
    if name == ".gitkeep" and path != "deploy/docker/.gitkeep":
        return True  # Only the explicitly reserved Docker directory is empty.
    if any(part in {".git", "node_modules", "target", "dist", ".temp", "__pycache__", "Password", "Notes"} for part in parts):
        return True
    if path.startswith(("deploy/local/", "third_party/", "vendor_projects/")):
        return True
    if name in {"AGENTS.md", "CLAUDE.md", "服务器连接方式.md", "settings.local.json"}:
        return True
    if name.startswith(".env") and not name.endswith(".example"):
        return True
    if name.endswith((".key", ".pem", ".p12", ".pfx", ".db", ".sqlite", ".sqlite3", "-wal", "-shm", ".log")):
        return True
    return name in {"server.env", "agent.env", "targets.json", "staging.json", "managed-targets.json"} or name.endswith(".known_hosts")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--staged", action="store_true", help="Check actual Git index contents instead of working files")
    args = parser.parse_args()
    listing = git("ls-files", "-z", "--cached", *( [] if args.staged else ["--others", "--exclude-standard"] ))
    paths = sorted(set(listing.decode().split("\0")) - {""})
    problems = []
    for path in paths:
        file = ROOT / path
        if not args.staged and not file.exists():
            continue  # intended deletion, confirmed when checking the staged index
        if forbidden(path):
            problems.append(f"{path}: private or generated file must not be tracked")
            continue
        content = git("show", ":" + path) if args.staged else file.read_bytes()
        for match in PRIVATE_KEY.finditer(content):
            line = content[:match.start()].count(b"\n") + 1
            problems.append(f"{path}:{line}: embedded private key")
        # Fixtures explicitly use fake database credentials. Examples must use
        # placeholders; scripts may interpolate variables, never production values.
        if not path.startswith(("tests/", "web/tests/")):
            production_content = content.split(b"#[cfg(test)]", 1)[0] if path.endswith(".rs") else content
            for match in DATABASE_CREDENTIAL.finditer(production_content):
                password = match.group(1)
                if b"$" in password or b"{" in password or password in {b"URL_ENCODED_PASSWORD", b"CHANGE_ME"}:
                    continue
                line = content[:match.start()].count(b"\n") + 1
                problems.append(f"{path}:{line}: literal database password requires review")
    for path in REQUIRED:
        if path not in paths or not (ROOT / path).is_file():
            problems.append(f"{path}: required release source missing from {'index' if args.staged else 'working tree'}")
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    print(f"Repository hygiene OK: {len(paths)} files checked; sources, locks and migrations present; no forbidden files/private keys found.")
    print("This is a targeted hygiene check, not an exhaustive secret scanner.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
