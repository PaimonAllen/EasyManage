#!/usr/bin/env python3
"""Bounded read-only disk analysis. Invoked as fixed code, never from remote files.

File metadata is aggregated in memory; only directory summaries leave the
scanner. No per-file database or temporary files. No symlink following, file
content, cleanup or shell execution. Stdout is batched NDJSON; stdin closes or
receives a byte to cancel. Results are lower bounds when incomplete.
"""
import errno
import json
import os
import pwd
import resource
import select
import signal
import stat
import sys
import time
from types import SimpleNamespace

MAX_DEPTH = 64
LOCAL_FS = {'ext2', 'ext3', 'ext4', 'xfs', 'btrfs', 'zfs', 'f2fs', 'jfs', 'reiserfs',
            'ntfs', 'ntfs3', 'fuseblk', 'exfat', 'vfat', 'hfsplus', 'bcachefs', 'overlay'}


class StopScan(Exception):
    pass


def mounts():
    result = []
    with open('/proc/self/mountinfo', encoding='utf-8') as stream:
        for line in stream:
            left, right = line.rstrip().split(' - ', 1)
            fields = left.split()
            def decode(value):
                for encoded, plain in [('\\040', ' '), ('\\011', '\t'), ('\\012', '\n'), ('\\134', '\\')]:
                    value = value.replace(encoded, plain)
                return value
            result.append({'id': int(fields[0]), 'device': fields[2], 'root': decode(fields[3]),
                           'path': decode(fields[4]), 'filesystem': right.split()[0]})
    return result


def contains(parent, child):
    return parent == child or child.startswith(parent.rstrip('/') + '/')


def filesystem_plan(table, allowed_fs, checkpoint):
    """Choose non-overlapping filesystem roots, preferring their original mount.

    Bind aliases refer to the same device/root and are not counted again. If a
    broader mount is unreadable, readable bind subtrees can still be collected.
    No directories are enumerated here; each root is revalidated when scanned.
    """
    selected, covered = [], {}
    for entry in sorted(table, key=lambda e: (e['root'].count('/'), len(e['root']), len(e['path']), e['path'])):
        checkpoint()
        if entry['filesystem'] not in allowed_fs:
            continue
        # Do not resolve paths through network or automount parents, even when
        # the last component was a local bind mount in the original snapshot.
        if any(p['filesystem'] not in allowed_fs | {'tmpfs', 'devtmpfs'} and
               p['path'] != entry['path'] and contains(p['path'], entry['path']) for p in table):
            continue
        key = (entry['device'], entry['filesystem'])
        if any(contains(root, entry['root']) for root in covered.get(key, [])):
            continue
        entry = dict(entry)
        fd = None
        try:
            if len(entry['path'].encode()) > 2048 or any(ord(c)<32 for c in entry['path']):
                raise ValueError('unsupported_mount_path')
            fd = open_directory(entry['path'])
            if mount_id(fd) != entry['id']:
                raise ValueError('mount_changed')
            with os.scandir(fd):
                pass
            covered.setdefault(key, []).append(entry['root'])
        except (OSError, ValueError):
            entry['unreadable'] = True
        finally:
            if fd is not None:
                os.close(fd)
        selected.append(entry)
    return selected


def mount_id(fd):
    with open('/proc/self/fdinfo/' + str(fd), encoding='ascii') as stream:
        for line in stream:
            if line.startswith('mnt_id:'):
                return int(line.split(':', 1)[1])
    raise ValueError('mount_identity_unavailable')


def open_directory(path):
    # Resolve every component with O_NOFOLLOW. realpath/lstat then open is racy.
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    fd = os.open('/', flags)
    try:
        for part in path.split('/'):
            if not part:
                continue
            new = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = new
        return fd
    except BaseException:
        os.close(fd)
        raise


def scan(path, seconds=0, emit=lambda value: None, cancelled=lambda: False,
         max_nodes=0, max_depth=MAX_DEPTH, allowed_fs=LOCAL_FS, stream=False, scope='directory'):
    if not path.startswith('/') or '\x00' in path or len(path.encode()) > 2048 or '..' in path.split('/'):
        raise ValueError('invalid_path')
    path = os.path.normpath(path)
    if scope not in ('directory', 'all_local') or (scope == 'all_local' and path != '/'):
        raise ValueError('invalid_scope')
    table = mounts()
    containing = sorted((entry for entry in table if contains(entry['path'], path)),
                        key=lambda entry: len(entry['path']))
    if scope == 'directory' and (not containing or containing[-1]['filesystem'] not in allowed_fs):
        raise ValueError('unsupported_filesystem')
    started = time.monotonic()
    if seconds < 0 or max_nodes < 0:
        raise ValueError('invalid_limit')
    deadline = started + seconds if seconds else None
    nodes, seen, pending = [], {}, {}
    # Streaming retains only the active directory stack and a small summary
    # batch. Ordinary files are discarded immediately after aggregation. Only
    # multiply-linked inode numbers need to survive for deduplication.
    directory_count = 0
    # Leave room to unwind active directories and report a partial result if
    # an unusually large hard-link set reaches the process memory ceiling.
    memory_reserve = bytearray(1024 * 1024)
    stats = {'visited': 0, 'errors': 0, 'skipped_mounts': 0, 'symlinks': 0, 'hardlinks': 0}
    reason, last_progress = '', started
    boundaries = {entry['path']: entry for entry in table}
    root_stat = root_mount = None
    plan = []
    filesystem_results = []

    def flush():
        if stream and pending:
            emit({'type': 'entries', 'nodes': list(pending.values())})
            pending.clear()

    def publish(node):
        if stream and node['kind'] == 'directory':
            pending[node['id']] = dict(node)
            if len(pending) >= 256:
                flush()

    def checkpoint(force=False):
        nonlocal reason, last_progress
        now = time.monotonic()
        if cancelled():
            reason = 'cancelled'
            raise StopScan()
        if deadline is not None and now >= deadline:
            reason = 'time_limit'
            raise StopScan()
        if max_nodes and stats['visited'] >= max_nodes:
            reason = 'entry_limit'
            raise StopScan()
        if force or now - last_progress >= 2:
            flush()
            emit({'type': 'progress', 'elapsed_seconds': round(now-started, 1), **stats})
            last_progress = now
        if stats['visited'] and stats['visited'] % 64 == 0:
            time.sleep(0.01)

    def add(name, parent, metadata, kind):
        nonlocal directory_count
        node = {'id': directory_count if kind == 'directory' else None, 'parent': parent, 'name': name, 'kind': kind,
                'allocated': max(0, metadata.st_blocks * 512), 'apparent': max(0, metadata.st_size),
                'files': int(kind == 'file'), 'directories': 0, 'issue': None, 'incomplete': False,
                'mtime': int(metadata.st_mtime), 'scan_path': None}
        if kind not in ('file', 'directory', 'symlink'):
            node['allocated'] = node['apparent'] = 0
            node['issue'] = 'special'
        if kind == 'file' and metadata.st_nlink > 1:
            inodes = seen.setdefault(metadata.st_dev, set())
            duplicate = metadata.st_ino in inodes
            inodes.add(metadata.st_ino)
            if duplicate:
                node['allocated'] = node['apparent'] = 0
                node['issue'] = 'hardlink'
                stats['hardlinks'] += 1
        if kind == 'symlink':
            node['issue'] = 'symlink'
            stats['symlinks'] += 1
        if kind == 'directory':
            directory_count += 1
            if not stream:
                nodes.append(node)
        stats['visited'] += 1
        return node

    def add_totals(parent, child):
        for key in ('allocated', 'apparent', 'files', 'directories'):
            parent[key] += child[key]
        if child['kind'] == 'directory':
            parent['directories'] += 1
        parent['incomplete'] |= child['incomplete']

    def skip_mount(child, entry=None):
        child['allocated'] = child['apparent'] = child['files'] = 0
        duplicate = scope == 'all_local' and entry and any(
            not p.get('unreadable') and p['id'] != entry['id'] and
            (p['device'], p['filesystem']) == (entry['device'], entry['filesystem']) and
            contains(p['root'], entry['root']) for p in plan)
        separate = scope == 'all_local' and entry and any(p['id'] == entry['id'] for p in plan)
        child['issue'] = 'duplicate_mount' if duplicate else 'separate_filesystem' if separate else 'mount_boundary'
        stats['skipped_mounts'] += 1

    def unavailable(node, issue):
        node['issue'] = issue
        node['incomplete'] = True
        stats['errors'] += 1

    def walk(fd, node, current, depth):
        nonlocal memory_reserve
        publish(node)
        try:
            if depth >= max_depth:
                unavailable(node, 'depth_limit')
                return
            try:
                with os.scandir(fd) as entries:
                    for entry in entries:
                        checkpoint()
                        name = entry.name
                        # JSON cannot represent surrogateescape filenames losslessly.
                        try:
                            name.encode('utf-8', 'strict')
                        except UnicodeError:
                            unavailable(node, 'unrepresentable_name')
                            continue
                        child_path = current.rstrip('/') + '/' + name
                        if child_path in boundaries:
                            # Do not stat known mount points: even metadata on
                            # a network/automount root can block or trigger I/O.
                            child = add(name, node['id'], empty, 'directory')
                            skip_mount(child, boundaries[child_path])
                            add_totals(node, child)
                            publish(child)
                            continue
                        try:
                            metadata = entry.stat(follow_symlinks=False)
                        except OSError:
                            unavailable(node, 'changed_or_denied')
                            continue
                        kind = 'directory' if stat.S_ISDIR(metadata.st_mode) else 'file' if stat.S_ISREG(metadata.st_mode) else 'symlink' if stat.S_ISLNK(metadata.st_mode) else 'special'
                        child = add(name, node['id'], metadata, kind)
                        child_fd = None
                        try:
                            if metadata.st_dev != root_stat.st_dev:
                                skip_mount(child)
                                continue
                            if kind != 'directory':
                                continue
                            if len(child_path.encode()) > 2048:
                                unavailable(child, 'path_limit')
                                continue
                            child_fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
                            actual = os.fstat(child_fd)
                            if (actual.st_dev, actual.st_ino) != (metadata.st_dev, metadata.st_ino):
                                unavailable(child, 'changed_or_denied')
                            elif mount_id(child_fd) != root_mount:
                                skip_mount(child)
                            else:
                                walk(child_fd, child, child_path, depth+1)
                        except OSError as error:
                            unavailable(child, 'permission_denied' if error.errno in (errno.EACCES, errno.EPERM) else 'changed_or_denied')
                        finally:
                            if child_fd is not None:
                                os.close(child_fd)
                            add_totals(node, child)
                            publish(child)
            except OSError:
                unavailable(node, 'changed_or_denied')
        except (StopScan, MemoryError):
            memory_reserve = None
            node['incomplete'] = True
            raise
        finally:
            publish(node)

    empty = SimpleNamespace(st_blocks=0, st_size=0, st_mtime=0)
    root = add(path, None, empty, 'directory')
    try:
        if stream:
            emit({'type': 'metadata', 'root': path, 'scope': scope, 'uid': os.geteuid(), 'username': pwd.getpwuid(os.geteuid()).pw_name})
        emit({'type': 'progress', 'elapsed_seconds': 0, **stats})
        if scope == 'all_local':
            publish(root)
            plan = filesystem_plan(table, allowed_fs, checkpoint)
            if not plan:
                unavailable(root, 'no_local_filesystems')
            targets = plan
        else:
            targets = [dict(containing[-1], path=path)]
        for entry in targets:
            checkpoint()
            node = add(entry['path'], root['id'], empty, 'directory') if scope == 'all_local' else root
            if scope == 'all_local':
                node['scan_path'] = entry['path']
            fd = None
            try:
                # Keep discovery decisions stable: an unreadable broad root
                # must not become readable here and overlap a fallback alias.
                if entry.get('unreadable'):
                    unavailable(node, 'changed_or_denied')
                    continue
                fd = open_directory(entry['path'])
                root_stat = os.fstat(fd)
                root_mount = mount_id(fd)
                if root_mount != entry['id']:
                    unavailable(node, 'mount_changed')
                else:
                    node.update(allocated=max(0, root_stat.st_blocks*512), apparent=max(0, root_stat.st_size), mtime=int(root_stat.st_mtime))
                    walk(fd, node, entry['path'], 0)
            except OSError as error:
                if scope == 'directory' and fd is None:
                    raise
                unavailable(node, 'permission_denied' if error.errno in (errno.EACCES, errno.EPERM) else 'changed_or_denied')
            finally:
                if fd is not None:
                    os.close(fd)
                if scope == 'all_local':
                    add_totals(root, node)
                    publish(node)
                    publish(root)
                    filesystem_results.append({'id':node['id'], 'path':entry['path'], 'filesystem':entry['filesystem'],
                                               'incomplete':node['incomplete']})
    except StopScan:
        root['incomplete'] = True
    except MemoryError:
        # The existing process memory ceiling also bounds extreme hard-link
        # sets. Return an explicitly partial snapshot instead of wrong totals.
        root['incomplete'] = True
        reason = 'memory_limit'
    seen.clear()
    publish(root)
    flush()
    return {'type': 'result', 'root': path, 'uid': os.geteuid(), 'username': pwd.getpwuid(os.geteuid()).pw_name,
            'status': 'cancelled' if reason == 'cancelled' else 'partial' if reason or root['incomplete'] else 'complete',
            'reason': reason or ('unreadable_entries' if root['incomplete'] else ''),
            'elapsed_seconds': round(time.monotonic()-started, 2),
            'scope': scope, 'filesystem': 'multiple' if scope == 'all_local' else containing[-1]['filesystem'],
            'filesystems': filesystem_results,
            **stats, 'detail_level': 'directories', 'saved_directories': directory_count,
            'paged': stream, 'nodes': [root] if stream else nodes}


def main():
    request = json.loads(sys.argv[1])
    mode = request['mode']
    if mode not in ('basic', 'full') or (os.geteuid() == 0) != (mode == 'full'):
        raise ValueError('privilege_mismatch')
    if mode == 'basic' and pwd.getpwuid(os.geteuid()).pw_name != request['username']:
        raise ValueError('identity_mismatch')
    resource.setrlimit(resource.RLIMIT_AS, (256*1024*1024, 256*1024*1024))
    resource.setrlimit(resource.RLIMIT_NOFILE, (192, 192))
    os.nice(19)
    stop = [False]
    signal.signal(signal.SIGTERM, lambda *_: stop.__setitem__(0, True))
    signal.signal(signal.SIGINT, lambda *_: stop.__setitem__(0, True))
    def cancelled():
        if stop[0]:
            return True
        if select.select([sys.stdin], [], [], 0)[0]:
            os.read(sys.stdin.fileno(), 1)
            return True
        return False
    def emit(value):
        print(json.dumps(value, ensure_ascii=True, separators=(',', ':')), flush=True)
    emit(scan(request['path'], request['seconds'], emit, cancelled,
              max_nodes=request.get('max_entries', 0), stream=True, scope=request.get('scope', 'directory')))


if __name__ == '__main__':
    try:
        main()
    except Exception:
        # Paths, credentials and filesystem exceptions never reach application logs.
        print('{"type":"error","message":"目录不可读取、身份不匹配或文件系统不支持；仅支持本地磁盘且路径不得包含符号链接"}', flush=True)
        sys.exit(1)
