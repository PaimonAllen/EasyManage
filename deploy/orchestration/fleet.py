#!/usr/bin/env python3
"""Trusted fleet orchestration with Linux user/system deployment backends."""
import argparse
import fcntl
from datetime import datetime, timezone
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[2]
BASE = Path.home() / ".easymanage"


def default_inventory():
    # Never run against the public example. Existing node IDs live in private data.
    return ROOT / "deploy/local/fleet.json"


def run(*args, capture=False, check=True, timeout=90):
    return subprocess.run(args, check=check, text=True, capture_output=capture, timeout=timeout)


def atomic_copy(source, dest, mode=0o600):
    dest.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = dest.with_name(dest.name + ".next")
    shutil.copyfile(source, temporary)
    temporary.chmod(mode)
    temporary.replace(dest)


def settings(path):
    # Read data only. Never evaluate a shell environment file.
    values = {}
    for line in path.read_text().splitlines():
        if line.strip() and not line.startswith("#") and "=" in line:
            key, value = line.split("=", 1)
            values[key] = value.strip().strip("\"'")
    return values


def parse_timestamp(value):
    # Ubuntu 22.04 Python 3.10 accepts at most microseconds; Chrono emits nanoseconds.
    return datetime.fromisoformat(re.sub(r"(\.\d{6})\d+", r"\1", value).replace("Z", "+00:00"))


def ssh_command(target):
    args = ["ssh", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes",
            "-o", "ConnectTimeout=8"]
    config = os.environ.get("EASYMANAGE_REMOTE_SSH_CONFIG")
    if config:
        args += ["-F", config]
    args.append(target)
    return args


def remote(target, command, **kwargs):
    return run(*ssh_command(target), command, **kwargs)


def require_linux(node):
    # Absence preserves old Linux inventories; no implicit cross-OS deployment.
    if node.get("platform", "linux") != "linux":
        raise ValueError("Fleet deployment currently supports Linux targets only")


def reject_persistent_cli(node):
    # Persistent sessions belong to the running controller. A separate CLI
    # process must not silently create extra SSH logins outside its budget.
    if node.get("connection_mode") == "ssh":
        raise RuntimeError("Persistent SSH target: use the platform GUI connection pool, not fleet SSH")
    for name in ("targets.json", "managed-targets.json"):
        path = BASE / "config" / name
        if path.exists():
            if path.is_symlink() or not path.is_file():
                raise RuntimeError("Unsafe target registry; refusing separate SSH")
            data = json.loads(path.read_text())
            targets = data if isinstance(data, list) else data["targets"]
            if any(target.get("node_id") == node.get("node_id") and target.get("connection_mode") == "ssh" for target in targets):
                raise RuntimeError("Persistent SSH target: use the platform GUI connection pool, not fleet SSH")


def node_preflight(node):
    require_linux(node)
    reject_persistent_cli(node)
    uuid.UUID(node["node_id"])
    ipaddress.ip_address(node["host"])
    if not isinstance(node["port"], int) or not 1 <= node["port"] <= 65535:
        raise ValueError("Invalid SSH port")
    if not re.fullmatch(r"[a-z_][a-z0-9_-]*", node["user"]):
        raise ValueError("Invalid deployment user")
    if node["user"] == "root":
        raise ValueError("Fleet deployment requires ordinary SSH users")
    if not re.fullmatch(r"[a-zA-Z0-9_.@-]+", node["ssh_target"]) or node["ssh_target"].startswith("-"):
        raise ValueError("Invalid SSH target")
    if node["ssh_target"] == "local":
        home, actual_user = str(Path.home()), run("id", "-un", capture=True).stdout.strip()
    else:
        resolved = run(*ssh_command(node["ssh_target"])[:-1], "-G", node["ssh_target"], capture=True)
        connection = dict(line.split(" ", 1) for line in resolved.stdout.splitlines() if " " in line)
        if connection.get("hostname") != node["host"] or int(connection.get("port", 22)) != node["port"]:
            raise ValueError(f"SSH alias and registry host/port disagree for {node['hostname']}")
        lines = remote(node["ssh_target"], "id -un; getent passwd \"$(id -un)\" | cut -d: -f6",
                       capture=True).stdout.strip().splitlines()
        actual_user, home = lines[-2:]
    if actual_user != node["user"] or not re.fullmatch(r"/home/[a-zA-Z0-9_.-]+", home):
        raise ValueError(f"SSH account/home mismatch for {node['hostname']}")
    node["agent_home"] = home + "/.easymanage"
    return node


def create_bundle(directory, node, enrollment, server_url, ca):
    require_linux(node)
    files = {
        "easymanage-agent": ROOT / "target/release/easymanage-agent",
        "install-user-agent.sh": ROOT / "deploy/linux/user/install-user-agent.sh",
        "agent-user-control.py": ROOT / "deploy/linux/user/agent-user-control.py",
        "install-full-candidate.sh": ROOT / "deploy/linux/operations/install-full-candidate.sh",
        "migrate-agent-to-user.sh": ROOT / "deploy/linux/migrations/migrate-agent-to-user.sh",
        "handover.sh": ROOT / "deploy/linux/migrations/handover.sh",
        "easymanage-agent.user.service": ROOT / "deploy/linux/user/easymanage-agent.user.service",
    }
    for name, source in files.items():
        atomic_copy(source, directory / name, 0o700 if name.endswith(".sh") or name == "easymanage-agent" else 0o600)
    (directory / "node.json").write_text(json.dumps(node, ensure_ascii=False))
    if enrollment and ca:
        atomic_copy(ca, directory / "ca.crt")
        config = {
            "EASYMANAGE_SERVER_URL": server_url,
            "EASYMANAGE_CA_CERTIFICATE": node["agent_home"] + "/config/ca.crt",
            "EASYMANAGE_AGENT_STATE_DB": node["agent_home"] + "/state/state.db",
            "EASYMANAGE_ENROLLMENT_TOKEN": enrollment,
            "EASYMANAGE_NODE_ID": node["node_id"],
            "EASYMANAGE_REPORT_INTERVAL_SECONDS": "10",
        }
        (directory / "agent.env").write_text("".join(f"{key}={value}\n" for key, value in config.items()))
        (directory / "agent.env").chmod(0o600)
    hashes = [f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n"
              for p in sorted(directory.iterdir()) if p.is_file()]
    (directory / "SHA256SUMS").write_text("".join(hashes))


def stage(node, enrollment, server_url, ca, prepare, full_candidate=False):
    reject_persistent_cli(node)
    staging = BASE / "staging"
    staging.mkdir(parents=True, exist_ok=True, mode=0o700)
    with tempfile.TemporaryDirectory(prefix="bundle-", dir=staging) as temporary:
        bundle = Path(temporary)
        create_bundle(bundle, node, enrollment, server_url, ca)
        if node["ssh_target"] == "local":
            destination = staging / ("agent." + uuid.uuid4().hex[:8])
            destination.mkdir(mode=0o700)
            for path in bundle.iterdir():
                atomic_copy(path, destination / path.name, path.stat().st_mode & 0o777)
        else:
            destination = remote(node["ssh_target"], 'umask 077; test ! -L "$HOME/.easymanage"; test ! -L "$HOME/.easymanage/staging"; mkdir -p "$HOME/.easymanage/staging"; mktemp -d "$HOME/.easymanage/staging/agent.XXXXXXXX"', capture=True).stdout.strip()
            if not re.fullmatch(re.escape(node["agent_home"]) + r"/staging/agent\.[a-zA-Z0-9]{8}", destination):
                raise ValueError("Unsafe remote staging path")
            scp = ["scp", "-q", "-p", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"]
            if os.environ.get("EASYMANAGE_REMOTE_SSH_CONFIG"):
                scp += ["-F", os.environ["EASYMANAGE_REMOTE_SSH_CONFIG"]]
            run(*scp, *(str(path) for path in bundle.iterdir()), f"{node['ssh_target']}:{destination}/")
        node["staging_directory"] = str(destination)
        admin_command = f"sudo bash {shlex.quote(str(destination) + '/handover.sh')} {shlex.quote(node['user'])}"
        if prepare:
            print(f"{node['hostname']}: private bundle ready at {destination}\n  Administrator handover: {admin_command}")
            return False
        command = ["bash", str(destination) + ("/install-full-candidate.sh" if full_candidate else "/install-user-agent.sh")]
        result = (run(*command, check=False) if node["ssh_target"] == "local"
                  else remote(node["ssh_target"], shlex.join(command), check=False))
        if result.returncode:
            print(f"{node['hostname']}: deployment needs attention (exit {result.returncode}); bundle retained.\n  One-time administrator handover: {admin_command}")
            return False
        print(f"{node['hostname']}: {'Full candidate staged' if full_candidate else 'basic user service started'}")
        # No recursive deletion of remote directories: remove just this known manifest's files.
        names = [path.name for path in bundle.iterdir()]
        if node["ssh_target"] == "local":
            for name in names:
                (destination / name).unlink()
            destination.rmdir()
        else:
            deletion = " && ".join(f"unlink {shlex.quote(str(destination) + '/' + name)}" for name in names)
            remote(node["ssh_target"], deletion + " && rmdir " + shlex.quote(destination))
        return True


def publish(manifest, inventory):
    for node in manifest["nodes"]:
        require_linux(node)
    if not (BASE / "config/server.env").is_file():
        raise RuntimeError("Platform user configuration is not initialized")
    release = BASE / "releases/current"
    names = ["target/release/easymanage-agent", "deploy/orchestration/fleet.py",
             "deploy/linux/user/agent-user-control.py",
             "deploy/linux/user/install-user-agent.sh", "deploy/linux/operations/install-full-candidate.sh",
             "deploy/linux/migrations/migrate-agent-to-user.sh", "deploy/linux/migrations/handover.sh",
             "deploy/linux/user/easymanage-agent.user.service"]
    for name in names:
        path = ROOT / name
        if not path.is_file(): raise RuntimeError(f"Release artifact missing: {path}")
        atomic_copy(path, release / name, 0o700 if name.endswith(".sh") or name.endswith(".py") or name.endswith("easymanage-agent") else 0o600)
    atomic_copy(inventory, release / "deploy/local/fleet.json")
    digest = hashlib.sha256((release / "target/release/easymanage-agent").read_bytes()).hexdigest()
    registry_path = BASE / "config/targets.json"
    existing = json.loads(registry_path.read_text())
    by_id = {node["node_id"]: node for node in manifest["nodes"]}
    for target in existing["targets"]:
        source = by_id.get(target["node_id"])
        if source is None or source["host"] != target["host"] or source["port"] != target["port"] or source["user"] != target["default_user"]:
            raise RuntimeError("Release inventory disagrees with the trusted target registry")
        if not re.fullmatch(r"[a-zA-Z0-9_.@-]+", source["ssh_target"]):
            raise RuntimeError("Invalid SSH alias in release inventory")
        target["ssh_target"] = source["ssh_target"]
        target["release_sha256"] = digest
    registry_path.parent.mkdir(parents=True, exist_ok=True)
    next_path = registry_path.with_name("targets.json.next")
    next_path.write_text(json.dumps(existing, indent=2))
    next_path.chmod(0o600)
    next_path.replace(registry_path)
    managed = registry_path.with_name("managed-targets.json")
    if managed.exists():
        descriptor = os.open(managed.with_name("managed-targets.lock"), os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, "a+") as lock:
            if os.fstat(lock.fileno()).st_uid != os.getuid() or os.fstat(lock.fileno()).st_mode & 0o077:
                raise RuntimeError("Managed target lock must be private")
            fcntl.flock(lock, fcntl.LOCK_EX)
            info = managed.lstat()
            if managed.is_symlink() or not managed.is_file() or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise RuntimeError("Managed target registry must be private and owned by the platform user")
            overlay = json.loads(managed.read_text())
            for target in overlay["targets"]:
                target["release_sha256"] = digest
            with tempfile.NamedTemporaryFile(mode="w", dir=managed.parent, prefix=".managed-publish-", delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(json.dumps(overlay, indent=2))
            temporary.chmod(0o600)
            temporary.replace(managed)
    print(f"Published Agent candidate SHA-256 {digest}; running services were not changed.")


def deploy_one(node_id, full_candidate):
    registry = json.loads((BASE / "config/targets.json").read_text())
    matches = [target for target in registry["targets"] if target["node_id"] == node_id]
    if len(matches) != 1: raise RuntimeError("Node is not in the trusted target registry")
    target = matches[0]
    require_linux(target)
    release_hash = hashlib.sha256((ROOT / "target/release/easymanage-agent").read_bytes()).hexdigest()
    if target.get("release_sha256") != release_hash:
        raise RuntimeError("Published release digest differs from the trusted registry")
    node = {"node_id": target["node_id"], "hostname": target["hostname"], "host": target["host"],
            "port": target["port"], "user": target["default_user"], "ssh_target": target["ssh_target"]}
    node_preflight(node)
    if node["agent_home"] != target["agent_home"]:
        raise RuntimeError("Remote home disagrees with the trusted target registry")
    config = settings(BASE / "config/server.env")
    ca = BASE / "config/ca.crt"
    enrollment = config.get("EASYMANAGE_ENROLLMENT_TOKEN")
    if not enrollment or not ca.is_file(): raise RuntimeError("Controller enrollment or CA configuration missing")
    success = stage(node, enrollment, target["server_url"], ca, False, full_candidate)
    if success and not full_candidate:
        target["agent_sha256"] = release_hash
        next_path = BASE / "config/targets.json.next"
        next_path.write_text(json.dumps(registry, indent=2))
        next_path.chmod(0o600)
        next_path.replace(BASE / "config/targets.json")
    return 0 if success else 1


def trusted_node(node_id):
    registry = json.loads((BASE / "config/targets.json").read_text())
    matches = [target for target in registry["targets"] if target["node_id"] == node_id]
    if len(matches) != 1: raise RuntimeError("Node is not in the trusted target registry")
    target = matches[0]
    manifest = json.loads(default_inventory().read_text())
    sources = [node for node in manifest["nodes"] if node["node_id"] == node_id]
    if len(sources) != 1: raise RuntimeError("Node is not in the published inventory")
    source = sources[0]
    require_linux(source)
    if any(source[key] != target[key2] for key, key2 in (("host", "host"), ("port", "port"), ("user", "default_user"), ("ssh_target", "ssh_target"))):
        raise RuntimeError("Published inventory disagrees with trusted target")
    node = {key: source[key] for key in ("node_id", "hostname", "host", "port", "user", "ssh_target")}
    node_preflight(node)
    if node["agent_home"] != target["agent_home"]:
        raise RuntimeError("Remote home disagrees with trusted target")
    return node


def restart_one(node_id):
    node = trusted_node(node_id)
    return 0 if control_agent(node, "restart") else 1


def record_full_update(node_id):
    registry_path = BASE / "config/targets.json"
    registry = json.loads(registry_path.read_text())
    matches = [target for target in registry["targets"] if target["node_id"] == node_id]
    if len(matches) != 1: raise RuntimeError("Node is not in the trusted target registry")
    target = matches[0]
    release_hash = hashlib.sha256((ROOT / "target/release/easymanage-agent").read_bytes()).hexdigest()
    if target.get("release_sha256") != release_hash:
        raise RuntimeError("Published candidate changed while recording Full update")
    target["agent_sha256"] = release_hash
    next_path = registry_path.with_name("targets.json.next")
    next_path.write_text(json.dumps(registry, indent=2))
    next_path.chmod(0o600)
    next_path.replace(registry_path)
    print(f"Recorded Full Agent candidate {release_hash} for {node_id}")
    return 0


def install_controller(allow_degraded=False, preserve_unit=False):
    config = BASE / "config/server.env"
    if not config.is_file():
        raise RuntimeError("User controller configuration missing; run the one-time administrator handover")
    backup = Path(tempfile.mkdtemp(prefix="easymanage-controller-backup.", dir=BASE))
    saved = {}
    destinations = [BASE / "bin/easymanage-server", BASE / "web/index.html",
                    Path.home() / ".config/systemd/user/easymanage-server.service"]
    docs_dist = ROOT / "docs/.vitepress/dist"
    destinations.extend(BASE / "web/guide" / path.relative_to(docs_dist)
                        for path in docs_dist.rglob("*") if path.is_file())
    for index, destination in enumerate(destinations):
        if destination.is_file():
            saved[destination] = backup / str(index)
            atomic_copy(destination, saved[destination], destination.stat().st_mode & 0o777)
    was_active = run("systemctl", "--user", "is-active", "--quiet", "easymanage-server.service", check=False).returncode == 0
    was_enabled = run("systemctl", "--user", "is-enabled", "--quiet", "easymanage-server.service", check=False).returncode == 0
    try:
        update_controller_files(preserve_unit=preserve_unit)
        context = ssl.create_default_context(cafile=str(BASE / "config/ca.crt"))
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
        for _ in range(20):
            try:
                with opener.open("https://127.0.0.1:8443/api/v1/health/ready", timeout=2) as response:
                    if response.status == 200 and run("systemctl", "--user", "is-active", "--quiet", "easymanage-server.service", check=False).returncode == 0:
                        return
            except (OSError, ValueError):
                pass
            if allow_degraded:
                try:
                    with opener.open("https://127.0.0.1:8443/api/v1/health/live", timeout=2) as response:
                        live = json.load(response)
                    with opener.open("https://127.0.0.1:8443/api/v1/operations/recovery", timeout=5) as response:
                        recovery = json.load(response)
                    if live.get("status") == "ok" and recovery.get("database_healthy") is False and recovery.get("platform_restart_available") is True and run("systemctl", "--user", "is-active", "--quiet", "easymanage-server.service", check=False).returncode == 0:
                        print("Controller HTTPS/recovery ready; PostgreSQL is unavailable and still requires administrator recovery.")
                        return
                except (OSError, ValueError):
                    pass
            time.sleep(1)
        raise RuntimeError("Updated controller did not become ready")
    except (OSError, RuntimeError, subprocess.SubprocessError):
        run("systemctl", "--user", "stop", "easymanage-server.service", check=False)
        for destination in destinations:
            if destination in saved:
                atomic_copy(saved[destination], destination, saved[destination].stat().st_mode & 0o777)
            elif destination.is_file():
                destination.unlink()
        run("systemctl", "--user", "daemon-reload", check=False)
        run("systemctl", "--user", "enable" if was_enabled else "disable", "easymanage-server.service", check=False)
        if was_active: run("systemctl", "--user", "start", "easymanage-server.service", check=False)
        raise
    finally:
        print(f"Controller rollback artifacts retained at {backup}")


def update_controller_files(preserve_unit=False):
    atomic_copy(ROOT / "target/release/easymanage-server", BASE / "bin/easymanage-server", 0o700)
    (BASE / "web").mkdir(exist_ok=True, mode=0o700)
    # Hashed assets are installed first and HTML last so in-flight browsers retain their assets.
    for path in (ROOT / "web/dist").rglob("*"):
        if path.is_file() and path.name != "index.html":
            atomic_copy(path, BASE / "web" / path.relative_to(ROOT / "web/dist"))
    atomic_copy(ROOT / "web/dist/index.html", BASE / "web/index.html")
    docs_dist = ROOT / "docs/.vitepress/dist"
    for path in docs_dist.rglob("*"):
        if path.is_file() and path.name != "index.html":
            atomic_copy(path, BASE / "web/guide" / path.relative_to(docs_dist))
    atomic_copy(docs_dist / "index.html", BASE / "web/guide/index.html")
    if not preserve_unit:
        atomic_copy(ROOT / "deploy/linux/user/easymanage-server.user.service",
                    Path.home() / ".config/systemd/user/easymanage-server.service")
        run("systemctl", "--user", "daemon-reload")
        run("systemctl", "--user", "enable", "easymanage-server.service")
    run("systemctl", "--user", "restart", "easymanage-server.service")


def verify_heartbeats(manifest, started):
    context = ssl.create_default_context(cafile=str(BASE / "config/ca.crt"))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
    deadline = time.monotonic() + 45
    expected = {node["node_id"] for node in manifest["nodes"]}
    while time.monotonic() < deadline:
        try:
            with opener.open(manifest["server_url"] + "/api/v1/nodes", timeout=4) as response:
                nodes = json.load(response)["nodes"]
            seen = {node["id"] for node in nodes if node["online"] and
                    ((node.get("metrics") or {}).get("privilege") or {}).get("mode") == "basic" and
                    parse_timestamp(node["metrics"]["collected_at"]) >= started}
            if expected <= seen:
                print("Controller and all ordinary-user agents have live basic-mode heartbeats.")
                return True
        except (OSError, ValueError, AttributeError, KeyError):
            pass
        time.sleep(2)
    print("Services installed, but not all fresh basic-mode heartbeats arrived within 45 seconds.")
    return False


def control_agent(node, action):
    require_linux(node)
    reject_persistent_cli(node)
    compat = ('[ -f "$HOME/.easymanage/config/agent-launch.json" ] && '
              'python3 -c \'import json; from pathlib import Path; d=json.loads((Path.home()/".easymanage/config/agent-launch.json").read_text()); raise SystemExit(0 if d.get("backend")=="compatibility" else 1)\'')
    # A promoted Full agent is intentionally not restarted/downgraded through a user service.
    if action == "restart":
        command = ("state=$(systemctl show easymanage-agent.service -p ActiveState --value); "
                   "if [ \"$state\" = active ] || systemctl is-enabled --quiet easymanage-agent.service; then "
                   "printf 'Full/system Agent requires administrator restart.\\n' >&2; exit 3; fi; "
                   f"if {compat}; then "
                   "python3 \"$HOME/.easymanage/bin/agent-user-control.py\" restart; exit $?; fi; "
                   "systemctl --user is-active --quiet easymanage-agent.service || exit 3; "
                   "systemctl --user restart easymanage-agent.service && "
                   "sleep 2 && systemctl --user is-active --quiet easymanage-agent.service")
        print(f"{node['hostname']}:", flush=True)
        if node["ssh_target"] == "local":
            return run("bash", "-c", command, check=False, timeout=35).returncode == 0
        return remote(node["ssh_target"], command, check=False, timeout=35).returncode == 0
    command = ("state=$(systemctl show easymanage-agent.service -p ActiveState --value); "
               "if [ \"$state\" = active ]; then "
               "printf 'System/Full agent is already active; Basic service is not started.\\n'; "
               "elif [ -n \"$state\" ] && [ \"$state\" != inactive ] && [ \"$state\" != failed ]; then "
               "printf 'System agent is transitioning; refusing to start a duplicate.\\n' >&2; exit 3; "
               "elif systemctl is-enabled --quiet easymanage-agent.service; then "
               "printf 'System/Full agent is configured but stopped; administrator action required.\\n' >&2; exit 3; "
               f"elif {compat}; then "
               f"python3 \"$HOME/.easymanage/bin/agent-user-control.py\" {action}; "
               f"else systemctl --user {action} easymanage-agent.service; fi")
    print(f"{node['hostname']}:", flush=True)
    if node["ssh_target"] == "local":
        return run("bash", "-c", command, check=False).returncode == 0
    return remote(node["ssh_target"], command, check=False).returncode == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["deploy", "controller", "prepare", "start", "status", "publish", "deploy-agent", "stage-full", "restart-agent", "record-full"])
    parser.add_argument("--inventory", type=Path, default=default_inventory())
    parser.add_argument("--node", type=str)
    parser.add_argument("--allow-degraded-controller", action="store_true", help="Explicitly allow controller recovery UI deployment while PostgreSQL is down")
    parser.add_argument("--preserve-controller-unit", action="store_true", help="Keep the installed platform unit and enablement; update artifacts and restart only")
    args = parser.parse_args()
    if sys.platform != "linux":
        raise RuntimeError("Fleet orchestration currently requires a Linux controller")
    if os.geteuid() == 0:
        raise RuntimeError("Run fleet orchestration as the ordinary SSH user")
    os.umask(0o077)
    if not args.inventory.is_file():
        raise RuntimeError("Private inventory missing: copy deploy/examples/fleet.example.json to deploy/local/fleet.json and configure your hosts and existing node IDs, or pass --inventory")
    manifest = json.loads(args.inventory.read_text())
    if manifest.get("example"):
        raise ValueError("Example inventory is not deployable; configure a private inventory and remove the example marker")
    for node in manifest["nodes"]:
        require_linux(node)
    if args.action == "publish":
        publish(manifest, args.inventory)
        return 0
    if args.action == "controller":
        for artifact in (ROOT / "target/release/easymanage-server", ROOT / "web/dist/index.html", ROOT / "docs/.vitepress/dist/index.html"):
            if not artifact.is_file(): raise RuntimeError(f"Build artifact missing: {artifact}")
        install_controller(allow_degraded=args.allow_degraded_controller, preserve_unit=args.preserve_controller_unit)
        print("Controller, console and VitePress updated; running Agents were not changed.")
        return 0
    if args.action in ("deploy-agent", "stage-full"):
        if not args.node: raise ValueError("--node is required")
        return deploy_one(str(uuid.UUID(args.node)), args.action == "stage-full")
    if args.action in ("restart-agent", "record-full"):
        if not args.node: raise ValueError("--node is required")
        node_id = str(uuid.UUID(args.node))
        return restart_one(node_id) if args.action == "restart-agent" else record_full_update(node_id)
    nodes = [node_preflight(node) for node in manifest["nodes"]]
    if args.action in ("start", "status"):
        action = "start" if args.action == "start" else "is-active"
        run("systemctl", "--user", action, "easymanage-server.service")
        outcomes = [control_agent(node, action) for node in nodes]
        return 0 if all(outcomes) else 1
    for artifact in (ROOT / "target/release/easymanage-agent", ROOT / "target/release/easymanage-server", ROOT / "web/dist/index.html", ROOT / "docs/.vitepress/dist/index.html"):
        if not artifact.is_file(): raise RuntimeError(f"Build artifact missing: {artifact}")
    config = BASE / "config/server.env"
    enrollment = settings(config).get("EASYMANAGE_ENROLLMENT_TOKEN") if config.is_file() else None
    ca = BASE / "config/ca.crt"
    if args.action == "deploy" and not enrollment:
        raise RuntimeError("First run --prepare-fleet and perform the administrator handover; no secrets are guessed")
    config_dir = BASE / "config"
    config_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    known_hosts = config_dir / "known_hosts"
    known = (Path.home() / ".ssh/known_hosts").read_text()
    for node in nodes:
        if node["ssh_target"] == "local":
            public_keys = "\n".join(path.read_text() for path in Path("/etc/ssh").glob("ssh_host_*_key.pub"))
        else:
            # Public host keys are retrieved over the already-verified SSH connection,
            # not unauthenticated ssh-keyscan. Pin every available algorithm for libssh2.
            public_keys = remote(node["ssh_target"],
                                 'for key in /etc/ssh/ssh_host_*_key.pub; do if [ -r "$key" ]; then cat "$key"; fi; done',
                                 capture=True).stdout
        host_label = node["host"] if node["port"] == 22 else f"[{node['host']}]:{node['port']}"
        count = 0
        for line in public_keys.splitlines():
            fields = line.split()
            if len(fields) >= 2 and re.fullmatch(r"(?:ssh-|ecdsa-)[a-zA-Z0-9@._+-]+", fields[0]) and re.fullmatch(r"[a-zA-Z0-9+/=]+", fields[1]):
                known += f"\n{host_label} {fields[0]} {fields[1]}\n"
                count += 1
        if not count:
            raise RuntimeError(f"No verified public host keys available for {node['hostname']}")
    known_hosts.write_text(known)
    known_hosts.chmod(0o600)
    digest = hashlib.sha256((ROOT / "target/release/easymanage-agent").read_bytes()).hexdigest()
    published = config_dir.parent / "releases/current/target/release/easymanage-agent"
    release_digest = hashlib.sha256(published.read_bytes()).hexdigest() if published.is_file() else None
    targets = [{key: node[key] for key in ("node_id", "hostname", "host", "port", "agent_home")} |
               {"default_user": node["user"], "known_hosts": str(known_hosts), "agent_sha256": digest,
                "ssh_target": node["ssh_target"],
                "release_sha256": digest if release_digest == digest else None,
                "server_url": manifest["server_url"],
                "ca_sha256": hashlib.sha256(ca.read_bytes()).hexdigest() if ca.is_file() else None}
               for node in nodes]
    (config_dir / "targets.json").write_text(json.dumps({"targets": targets}, indent=2))
    started = datetime.now(timezone.utc)
    if args.action == "deploy": install_controller()
    outcomes = [stage(node, enrollment, manifest["server_url"], ca if ca.is_file() else None, args.action == "prepare") for node in nodes]
    (config_dir / "staging.json").write_text(json.dumps({"nodes": nodes}, indent=2))
    if args.action == "prepare":
        print("Staging only completed. No running services were changed.")
        return 0
    return 0 if all(outcomes) and verify_heartbeats(manifest, started) else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        print(f"Fleet operation failed: {error}", file=sys.stderr)
        sys.exit(1)
