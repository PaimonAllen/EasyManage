#!/usr/bin/env python3
"""Publish/install a bounded platform-only candidate; never touch Agent state.

The browser uses an embedded copy of this installer, not candidate-supplied code.
Run publish after building. Install runs as the platform owner in a detached unit.
Database migrations are forward-only: file rollback cannot undo schema changes.
"""
import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import ssl
import stat
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

BASE = Path.home() / ".local/share/easymanage"
MAX_FILES = 20000
MAX_BYTES = 512 * 1024 * 1024


def private(path, directory=False):
    info = path.lstat()
    if info.st_uid != os.getuid() or info.st_mode & 0o022:
        raise ValueError("候选目录或文件所有者/写权限不安全")
    if not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise ValueError("不接受符号链接或特殊文件")
    return info


def safe_parent(path):
    for parent in [path, *path.parents]:
        info = parent.lstat()
        if not stat.S_ISDIR(info.st_mode):
            raise ValueError("部署路径不接受符号链接")
        if info.st_uid not in (Path("/").stat().st_uid, os.getuid()):
            raise ValueError("部署路径包含不可信所有者")
        if info.st_mode & 0o002 and not info.st_mode & stat.S_ISVTX:
            raise ValueError("部署路径包含公开可写目录")
        # The operator permits shared home/.local directories. Keep the managed
        # platform root private, without rejecting ancestor group-write bits.
        if parent == BASE and info.st_mode & 0o022:
            raise ValueError("平台自身目录不可由组/其他账户写入")


def digest(path):
    if private(path).st_size > MAX_BYTES:
        raise ValueError("候选文件超出大小限制")
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest() if hasattr(hashlib, "file_digest") else hash_stream(stream)


def hash_stream(stream):
    value = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        value.update(block)
    return value.hexdigest()


def write_json(path, value):
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".platform-", delete=False) as stream:
        temporary = Path(stream.name)
        stream.write(json.dumps(value, ensure_ascii=False, sort_keys=True).encode())
    temporary.chmod(0o600)
    temporary.replace(path)


def destination(name):
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or str(path) != name:
        raise ValueError("候选路径不合法")
    if name == "bin/easymanage-server" or name.startswith("web/"):
        return BASE / name
    raise ValueError("候选包仅允许平台二进制和页面")


def candidate(expected=None, verify=False):
    safe_parent(BASE)
    releases = BASE / "platform-releases"
    private(releases, True)
    pointer = releases / "candidate.json"
    if private(pointer).st_size > 1024:
        raise ValueError("候选索引过大")
    data = json.loads(pointer.read_text())
    release_id = str(uuid.UUID(data["id"]))
    root = releases / release_id
    private(root, True)
    manifest_path = root / "manifest.json"
    if private(manifest_path).st_size > 4 * 1024 * 1024:
        raise ValueError("候选清单过大")
    raw = manifest_path.read_bytes()
    sha = hashlib.sha256(raw).hexdigest()
    if sha != data["sha256"] or expected is not None and sha != expected:
        raise ValueError("候选包已改变，请重新确认版本")
    manifest = json.loads(raw)
    files = manifest["files"]
    if not isinstance(files, dict) or not 3 <= len(files) <= MAX_FILES:
        raise ValueError("候选包文件数量不合法")
    for required in ("bin/easymanage-server", "web/index.html", "web/guide/index.html"):
        if required not in files:
            raise ValueError("候选包缺少平台、控制台或操作手册")
    total = 0
    for name, entry in files.items():
        destination(name)
        if entry["mode"] != (0o700 if name == "bin/easymanage-server" else 0o600):
            raise ValueError("候选文件模式不合法")
        if verify:
            source = root / name
            safe_parent(source.parent)
            total += private(source).st_size
            if total > MAX_BYTES or digest(source) != entry["sha256"]:
                raise ValueError("候选完整性校验失败")
    return root, manifest, sha


def publish(source):
    safe_parent(BASE)
    releases = BASE / "platform-releases"
    releases.mkdir(mode=0o700, exist_ok=True)
    private(releases, True)
    root = releases / str(uuid.uuid4())
    root.mkdir(mode=0o700)
    files, total = {}, 0
    inputs = [(source / "target/release/easymanage-server", "bin/easymanage-server")]
    for folder, prefix in [(source / "web/dist", "web"), (source / "docs/.vitepress/dist", "web/guide")]:
        if not (folder / "index.html").is_file():
            raise ValueError("请先构建平台、控制台和操作手册")
        for path in sorted(folder.rglob("*")):
            if path.is_symlink():
                raise ValueError("构建产物不接受符号链接")
            if path.is_file():
                inputs.append((path, prefix + "/" + path.relative_to(folder).as_posix()))
    if len(inputs) > MAX_FILES:
        raise ValueError("候选文件数量超限")
    for path, name in inputs:
        if path.is_symlink() or not path.is_file():
            raise ValueError("构建产物不完整")
        total += path.stat().st_size
        if total > MAX_BYTES:
            raise ValueError("候选包大小超限")
        destination(name)
        output = root / name
        output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        shutil.copyfile(path, output)
        mode = 0o700 if name.startswith("bin/") else 0o600
        output.chmod(mode)
        files[name] = {"sha256": digest(output), "mode": mode}
    manifest = {"files": files, "published_at": datetime.now(timezone.utc).isoformat()}
    write_json(root / "manifest.json", manifest)
    sha = digest(root / "manifest.json")
    write_json(releases / "candidate.json", {"id": root.name, "sha256": sha})
    print("平台候选已发布；运行中的平台和 Agent 均未改变。 SHA-256 " + sha)


def status():
    result = {"update_available": False, "candidate_sha256": None, "published_at": None,
              "message": "尚未发布平台候选包"}
    try:
        root, manifest, sha = candidate()
        # Compare actual installed platform and console, not version strings.
        current = all((BASE / name).is_file() and digest(BASE / name) == entry["sha256"] for name, entry in manifest["files"].items()
                      if name in ("bin/easymanage-server", "web/index.html", "web/guide/index.html"))
        result.update(update_available=not current, candidate_sha256=sha,
                      candidate_server_sha256=manifest["files"]["bin/easymanage-server"]["sha256"],
                      published_at=manifest["published_at"], message="已是候选版本" if current else "有平台更新可安装")
    except FileNotFoundError:
        pass
    except (ValueError, KeyError, TypeError, OSError):
        result["message"] = "平台候选或部署目录不安全/不完整，请管理员检查后重新发布"
    outcome = BASE / "platform-update-result.json"
    try:
        if private(outcome).st_size <= 4096:
            result["last_result"] = json.loads(outcome.read_text())
    except (OSError, ValueError):
        pass
    return result


def copy_atomic(source, target, mode):
    target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    safe_parent(target.parent)
    if target.exists() or target.is_symlink():
        private(target)
    with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".platform-", delete=False) as stream:
        temporary = Path(stream.name)
        with source.open("rb") as original:
            shutil.copyfileobj(original, stream)
    temporary.chmod(mode)
    temporary.replace(target)


def service(*args):
    return subprocess.run(["/usr/bin/systemctl", "--user", *args], check=True,
                          capture_output=True, timeout=30)


def ready(url):
    context = ssl.create_default_context(cafile=str(BASE / "config/ca.crt"))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
    for _ in range(30):
        try:
            with opener.open(url + "/api/v1/health/ready", timeout=2) as response:
                if response.status == 200:
                    service("is-active", "--quiet", "easymanage-server.service")
                    return
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
        time.sleep(1)
    raise RuntimeError("平台未恢复就绪")


def install(expected, url):
    if os.geteuid() == 0 or sys.platform != "linux":
        raise ValueError("仅支持 Linux 普通用户平台")
    # The independent system transient unit survives termination of the old server.
    safe_parent(BASE)
    lock_path = BASE / "platform-update.lock"
    if lock_path.exists() or lock_path.is_symlink():
        private(lock_path)
    with lock_path.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        root, manifest, sha = candidate(expected, verify=True)
        service("is-active", "--quiet", "easymanage-server.service")
        backup = Path(tempfile.mkdtemp(prefix="platform-backup-", dir=BASE))
        saved = {}
        # Preflight and backup every replacement before any installed file changes.
        for name in manifest["files"]:
            target = destination(name)
            existing_parent = target.parent
            while not existing_parent.exists():
                existing_parent = existing_parent.parent
            safe_parent(existing_parent)
            if target.exists() or target.is_symlink():
                mode = stat.S_IMODE(private(target).st_mode)
                output = backup / name
                output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                shutil.copyfile(target, output)
                output.chmod(mode)
                saved[name] = mode
        write_json(BASE / "platform-update-result.json", {"status": "installing", "sha256": sha})
        try:
            # Hashed assets first, HTML entry points last; old assets are retained.
            names = sorted(manifest["files"], key=lambda name: name.endswith("index.html"))
            for name in names:
                copy_atomic(root / name, destination(name), manifest["files"][name]["mode"])
            service("restart", "easymanage-server.service")
            ready(url)
            write_json(BASE / "platform-update-result.json", {"status": "success", "sha256": sha,
                       "message": "平台已更新并通过就绪检查；Agent 未改变"})
        except Exception:
            # Attempt every restore even when stopping the failed service itself fails.
            restored = True
            try:
                service("stop", "easymanage-server.service")
            except Exception:
                pass
            for name, mode in saved.items():
                try:
                    copy_atomic(backup / name, destination(name), mode)
                except Exception:
                    restored = False
            # Retain new hashed assets; only restore entry points and binary. No recursive deletion.
            try:
                service("restart", "easymanage-server.service")
                ready(url)
                message = "更新失败，平台文件已回退并恢复就绪；数据库迁移不回退" if restored else "更新失败，文件回退不完整，需要管理员检查已保留的备份；数据库迁移不回退"
            except Exception:
                message = "更新失败，自动恢复未完成，需要管理员检查已保留的备份与服务；数据库迁移不回退"
            write_json(BASE / "platform-update-result.json", {"status": "failed", "sha256": sha, "message": message})
            raise RuntimeError(message) from None


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("publish", "status", "check", "scheduled", "install"))
    parser.add_argument("--source", type=Path)
    parser.add_argument("--sha256")
    parser.add_argument("--url")
    args = parser.parse_args()
    if args.action == "publish":
        if os.geteuid() == 0 or not args.source:
            raise ValueError("以平台普通用户从已构建源码目录发布")
        publish(args.source.resolve())
    elif args.action == "status":
        print(json.dumps(status(), ensure_ascii=False))
    elif args.action == "check":
        if not args.sha256:
            raise ValueError("需要候选摘要")
        candidate(args.sha256, verify=True)
    elif args.action == "scheduled":
        candidate(args.sha256)
        write_json(BASE / "platform-update-result.json", {"status": "scheduled", "sha256": args.sha256,
                   "message": "平台更新已安排，等待安装与就绪检查"})
    else:
        if not args.sha256 or not args.url:
            raise ValueError("需要已确认的候选摘要与本地就绪地址")
        install(args.sha256, args.url)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        if len(sys.argv) > 1 and sys.argv[1] == "install":
            try:
                safe_parent(BASE)
                outcome = BASE / "platform-update-result.json"
                private(outcome)
                previous = json.loads(outcome.read_text())
                if previous.get("status") in ("scheduled", "installing"):
                    previous.update(status="failed", message="平台更新未完成，请管理员检查服务日志与已保留的回退文件")
                    write_json(outcome, previous)
            except Exception:
                pass
        # No config contents, database URL, SSH terminal output or secrets in logs.
        print("平台更新操作失败，请核对候选完整性、部署权限和平台服务日志。", file=sys.stderr)
        sys.exit(1)
