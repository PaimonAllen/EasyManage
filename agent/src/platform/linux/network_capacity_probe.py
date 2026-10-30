"""One capacity query in an isolated child. No directory enumeration or writes to the mount."""
import fcntl
from datetime import datetime, timezone
import json
import os
import stat
import sys


def decode(value):
    for before, after in ((r"\040", " "), (r"\011", "\t"), (r"\012", "\n"), (r"\134", "\\")):
        value = value.replace(before, after)
    return value


def source(value):
    if value.startswith("//"):
        authority, _, path = value[2:].partition("/")
        return "//" + authority.rsplit("@", 1)[-1] + "/" + path
    if ":/" in value:
        authority, path = value.split(":/", 1)
        return authority.rsplit("@", 1)[-1] + ":/" + path
    return value


def identity(path):
    # Read procfs only. Inspecting a mount must not itself touch its remote server.
    with open("/proc/self/mountinfo") as stream:
        contents = stream.read(1024 * 1024 + 1)
    if len(contents) > 1024 * 1024:
        raise ValueError("inventory too large")
    found = None
    for line in contents.splitlines():
        left, right = line.split(" - ", 1)
        fields, rest = left.split(), right.split()
        if decode(fields[4]) == path and (found is None or int(fields[0]) > found[0]):
            found = (int(fields[0]), fields[2], decode(fields[3]), rest[0], source(decode(rest[1])))
    return found


def run(request):
    # A killed process in uninterruptible NFS I/O may survive a daemon restart.
    # Keep a persistent inode locked across the syscall; never unlink this file.
    descriptor = os.open(request["lock"], os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    with os.fdopen(descriptor, "rb") as lock:
        info = os.fstat(lock.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            return {"status": "error"}
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return {"status": "busy"}
        before = identity(request["path"])
        expected = (request["device"], request["root"], request["filesystem"], request["source"])
        if before is None or before[1:] != expected:
            return {"status": "changed"}
        stats = os.statvfs(request["path"])
        if identity(request["path"]) != before:
            return {"status": "changed"}
        total = stats.f_blocks * stats.f_frsize
        available = max(0, stats.f_bavail) * stats.f_frsize
        if not 0 < total <= 2**64 - 1 or not 0 <= available <= total:
            return {"status": "error"}
        return {"status": "ok", "total_bytes": total, "available_bytes": available,
                "collected_at": datetime.now(timezone.utc).isoformat()}


if __name__ == "__main__":
    try:
        result = run(json.loads(sys.argv[1]))
    except Exception:
        # No exception text, mount options, inherited environment or credentials in output.
        result = {"status": "error"}
    print(json.dumps(result))
