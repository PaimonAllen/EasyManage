#!/usr/bin/env python3
"""Unprivileged fallback supervisor. No systemd user manager or sudo required."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time

BASE = Path.home() / ".easymanage"
BEGIN = "# BEGIN EasyManage Agent compatibility"
END = "# END EasyManage Agent compatibility"
MAX_LOG = 1024 * 1024


def safe(path):
    for item in [path, *path.parents]:
        if item.is_symlink():
            raise RuntimeError("Refusing a symlink in the user deployment")
    if path.exists() and path.stat().st_uid != os.getuid():
        raise RuntimeError("Deployment path belongs to another account")


def prepare():
    if os.geteuid() == 0:
        raise RuntimeError("Compatibility launcher must run as the ordinary SSH user")
    for name in ("", "state", "logs", "config", "bin"):
        path = BASE / name
        safe(path)
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    for name in ("state/compat.enabled", "state/compat.lock", "state/compat-control.lock", "state/compat-process.json", "logs/agent.log", "logs/agent.log.1", "config/agent-launch.json"):
        safe(BASE / name)


def write_json(path, value):
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".agent-", delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(json.dumps(value).encode())
    temporary.chmod(0o600)
    temporary.replace(path)


def identity(pid):
    try:
        return Path(f"/proc/{int(pid)}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except (OSError, ValueError, IndexError):
        return None


def process():
    try:
        record = json.loads((BASE / "state/compat-process.json").read_text())
        pid = int(record["pid"])
        command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        if pid > 1 and identity(pid) == record["start"] and str(BASE / "bin/agent-user-control.py").encode() in command and b"supervise" in command:
            return record
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return None


def orphan():
    try:
        record = json.loads((BASE / "state/compat-process.json").read_text())
        pid = int(record["agent_pid"])
        command = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        if pid > 1 and identity(pid) == record["agent_start"] and Path(f"/proc/{pid}").stat().st_uid == os.getuid() and str(BASE / "bin/easymanage-agent").encode() in command:
            return record
    except (OSError, ValueError, KeyError, TypeError):
        pass
    return None


def full_guard():
    if not shutil.which("systemctl"):
        for path in ("/usr/local/sbin/easymanage-agent", "/var/lib/easymanage-agent"):
            if Path(path).exists():
                raise RuntimeError("Existing system Agent requires administrator reconciliation")
        return
    for action in ("is-active", "is-enabled"):
        result = subprocess.run(["systemctl", action, "--quiet", "easymanage-agent.service"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        if result.returncode == 0:
            raise RuntimeError("Full/system Agent is present; refusing to start Basic")


def cron_table():
    if not shutil.which("crontab"):
        return None
    result = subprocess.run(["crontab", "-l"], capture_output=True, text=True, timeout=5, env=os.environ | {"LC_ALL":"C"})
    if result.returncode == 0:
        return result.stdout
    if result.returncode == 1 and result.stderr.strip().startswith("no crontab for "):
        return ""
    return None  # Do not replace a table we cannot read.


def with_managed_cron(table):
    lines = table.splitlines()
    if lines.count(BEGIN) != lines.count(END) or lines.count(BEGIN) > 1:
        raise RuntimeError("Ambiguous managed cron block; left unchanged")
    if BEGIN in lines:
        start, end = lines.index(BEGIN), lines.index(END)
        if end < start:
            raise RuntimeError("Invalid managed cron block; left unchanged")
        lines = lines[:start] + lines[end + 1:]
    command = "/usr/bin/python3 " + shlex.quote(str(BASE / "bin/agent-user-control.py")) + " start --enabled-only >/dev/null 2>&1"
    # The enabled marker prevents cron resurrecting Basic after promotion/stop.
    return "\n".join([*lines, BEGIN, "@reboot " + command, "* * * * * " + command, END]) + "\n"


def configure():
    cron = False
    try:
        table = cron_table()
    except (OSError, subprocess.TimeoutExpired):
        table = None
    if table is not None:
        try:
            candidate = with_managed_cron(table)
            cron = subprocess.run(["crontab", "-"], input=candidate, text=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5).returncode == 0
        except (RuntimeError, OSError, subprocess.TimeoutExpired):
            pass
    write_json(BASE / "config/agent-launch.json", {"backend":"compatibility", "autostart":"cron" if cron else "manual"})


def start(enabled_only=False):
    if enabled_only and not (BASE / "state/compat.enabled").is_file():
        return
    full_guard()
    if process() or orphan():
        return
    (BASE / "state/compat.enabled").touch(mode=0o600)
    env = {"PATH":os.environ.get("PATH", "/usr/sbin:/usr/bin:/sbin:/bin"), "HOME":str(Path.home()), "USER":os.environ.get("USER", "")}
    subprocess.Popen([sys.executable, str(BASE / "bin/agent-user-control.py"), "supervise"], env=env,
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True, close_fds=True)
    deadline = time.monotonic() + 4
    while time.monotonic() < deadline:
        if process():
            return
        time.sleep(0.05)
    raise RuntimeError("Compatibility supervisor did not start; inspect ~/.easymanage/logs")


def stop(timeout=20):
    (BASE / "state/compat.enabled").unlink(missing_ok=True)
    record = process()
    orphan_record = orphan() if not record else None
    record = record or orphan_record
    if not record:
        return
    os.kill(record["agent_pid"] if orphan_record else record["pid"], signal.SIGTERM)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process() and not orphan():
            return
        time.sleep(0.1)
    raise RuntimeError("Supervisor shutdown timed out; identity files left unchanged")


def pump(stream):
    log = BASE / "logs/agent.log"
    with log.open("ab", buffering=0) as output:
        while data := stream.read1(8192):
            if output.tell() + len(data) > MAX_LOG:
                output.close()
                log.replace(BASE / "logs/agent.log.1")
                output = log.open("ab", buffering=0)
            output.write(data)
        output.close()


def supervise():
    with (BASE / "state/compat.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        stopped = threading.Event()
        for sig in (signal.SIGTERM, signal.SIGINT):
            signal.signal(sig, lambda *_: stopped.set())
        signal.signal(signal.SIGHUP, signal.SIG_IGN)
        write_json(BASE / "state/compat-process.json", {"pid":os.getpid(), "start":identity(os.getpid())})
        try:
            while not stopped.is_set() and (BASE / "state/compat.enabled").is_file():
                full_guard()
                binary = BASE / "bin/easymanage-agent"
                safe(binary)
                env = os.environ | {"EASYMANAGE_ENV_FILE":str(BASE / "config/agent.env"), "EASYMANAGE_ADMIN_POLICY":"warn", "RUST_LOG":"warn"}
                # The Agent inherits the singleton lock. Killing only this
                # supervisor cannot let cron launch a duplicate live collector.
                child = subprocess.Popen([str(binary)], cwd=BASE, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, close_fds=True, pass_fds=(lock.fileno(),), start_new_session=True)
                write_json(BASE / "state/compat-process.json", {"pid":os.getpid(), "start":identity(os.getpid()), "agent_pid":child.pid, "agent_start":identity(child.pid)})
                reader = threading.Thread(target=pump, args=(child.stdout,), daemon=True)
                reader.start()
                try:
                    while child.poll() is None and not stopped.wait(1):
                        pass
                finally:
                    if child.poll() is None:
                        os.killpg(child.pid, signal.SIGTERM)
                        try:
                            child.wait(timeout=15)
                        except subprocess.TimeoutExpired:
                            os.killpg(child.pid, signal.SIGKILL)
                            child.wait(timeout=3)
                    reader.join(timeout=2)
                    if not reader.is_alive(): child.stdout.close()
                if stopped.wait(10):
                    break
        finally:
            (BASE / "state/compat-process.json").unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["configure", "start", "stop", "restart", "status", "describe", "supervise"])
    parser.add_argument("--enabled-only", action="store_true")
    parser.add_argument("--timeout", type=int, default=20)
    args = parser.parse_args()
    os.umask(0o077)
    prepare()
    if args.action in ("configure", "start", "stop", "restart"):
        with (BASE / "state/compat-control.lock").open("a") as control:
            fcntl.flock(control, fcntl.LOCK_EX)
            if args.action == "configure": configure()
            elif args.action == "start": start(args.enabled_only)
            elif args.action == "stop": stop(args.timeout)
            else: stop(args.timeout); start()
    elif args.action == "status": raise SystemExit(0 if process() or orphan() else 3)
    elif args.action == "describe": print((BASE / "config/agent-launch.json").read_text())
    elif args.action == "supervise": supervise()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired):
        # Fixed text only; no inherited environment/credentials or process output.
        print("Compatibility Agent operation failed; inspect the private deployment and system policy.", file=sys.stderr)
        raise SystemExit(1)
