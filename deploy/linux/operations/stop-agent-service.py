#!/usr/bin/env python3
"""Fixed Agent-only shutdown. Retain installation/identity/history; no start action."""
import json
import os
from pathlib import Path
import pwd
import re
import sqlite3
import subprocess
import sys
import uuid

UNIT = 'easymanage-agent.service'


def regular(path, owner, limit=65536):
    if path.is_symlink() or not path.is_file() or path.stat().st_uid != owner or path.stat().st_size > limit:
        raise RuntimeError('Untrusted Agent file')
    return path


def identity(config, state, expected, owner):
    values = {}
    for line in regular(config, owner).read_text().splitlines():
        if line.strip() and not line.startswith('#') and '=' in line:
            key, value = line.split('=', 1)
            values[key] = value.strip().strip('\"\'')
    known = []
    if values.get('EASYMANAGE_NODE_ID'):
        known.append(uuid.UUID(values['EASYMANAGE_NODE_ID']))
    if values.get('EASYMANAGE_AGENT_STATE_DB', str(state)) != str(state):
        raise RuntimeError('Unexpected identity database path')
    if state.exists():
        regular(state, owner, 1 << 50)
        with sqlite3.connect(state.as_uri() + '?mode=ro', uri=True, timeout=2) as database:
            row = database.execute("SELECT value FROM metadata WHERE key='node_id'").fetchone()
        if row:
            known.append(uuid.UUID(row[0]))
    if not known or any(value != expected for value in known):
        raise RuntimeError('Agent identity mismatch')


def run(args, expected=(0,)):
    result = subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=35)
    if result.returncode not in expected or len(result.stdout) > 65536:
        raise RuntimeError('Agent service manager refused shutdown')
    return result.stdout.strip()


def unit_state(prefix, binary, full):
    def show(field):
        return run(prefix + ['show', UNIT, '-p', field, '--value'])
    if show('LoadState') != 'loaded':
        if full:
            raise RuntimeError('Full Agent unit unavailable')
        return False
    executable = re.search(r'(?:^|[ {;])path=([^ ;]+)', show('ExecStart'))
    if not executable or executable[1] != str(binary) or full and show('User') != 'root':
        raise RuntimeError('Refusing another service or privilege mode')
    return True


def stop(mode, node, home, owner):
    expected = uuid.UUID(node)
    if mode == 'full':
        if os.geteuid() != 0:
            raise RuntimeError('Administrator required')
        identity(Path('/etc/easymanage/agent.env'), Path('/var/lib/easymanage-agent/state.db'), expected, 0)
        prefix = ['/usr/bin/systemctl']
        unit_state(prefix, Path('/usr/local/sbin/easymanage-agent'), True)
        run(prefix + ['disable', '--now', UNIT])
        if run(prefix + ['show', UNIT, '-p', 'ActiveState', '--value']) not in ('inactive', 'failed') or run(prefix + ['show', UNIT, '-p', 'MainPID', '--value']) != '0':
            raise RuntimeError('Full Agent still running')
        if run(prefix + ['is-enabled', UNIT], expected=(0, 1, 3, 4)) not in ('disabled', 'masked', 'masked-runtime', 'static', 'indirect'):
            raise RuntimeError('Agent autostart still enabled')
    elif mode == 'basic':
        account = pwd.getpwuid(os.geteuid())
        base = Path(home)
        if os.geteuid() == 0 or account.pw_name != owner or base != Path(account.pw_dir) / '.easymanage':
            raise RuntimeError('Original ordinary account required')
        for path in [base, *base.parents]:
            if path.is_symlink():
                raise RuntimeError('Unsafe deployment path')
        identity(base / 'config/agent.env', base / 'state/state.db', expected, os.geteuid())
        system = ['/usr/bin/systemctl']
        full_state = subprocess.run(system + ['show', UNIT, '-p', 'ActiveState', '--value'], capture_output=True, text=True, timeout=5)
        full_enabled = subprocess.run(system + ['is-enabled', '--quiet', UNIT], capture_output=True, timeout=5)
        if full_state.returncode == 0 and full_state.stdout.strip() not in ('inactive', '') or full_enabled.returncode == 0:
            raise RuntimeError('A Full service exists; administrator authorization required')
        prefix = ['/usr/bin/systemctl', '--user']
        # Stop the compatibility watchdog before disabling an optional user unit.
        launcher = base / 'bin/agent-user-control.py'
        if launcher.exists():
            regular(launcher, os.geteuid())
            run(['/usr/bin/python3', str(launcher), 'stop', '--timeout', '25'])
            result = subprocess.run(['/usr/bin/python3', str(launcher), 'status'], capture_output=True, timeout=5)
            if result.returncode != 3 or (base / 'state/compat.enabled').exists():
                raise RuntimeError('Compatibility Agent still enabled')
        launch = base / 'config/agent-launch.json'
        backend = json.loads(regular(launch, os.geteuid()).read_text()).get('backend') if launch.exists() else 'systemd'
        if backend not in ('systemd', 'compatibility'):
            raise RuntimeError('Unknown launcher')
        manager = subprocess.run(prefix + ['show', UNIT, '-p', 'LoadState', '--value'], capture_output=True, text=True, timeout=5)
        if manager.returncode == 0 and manager.stdout.strip() == 'loaded':
            unit_state(prefix, base / 'bin/easymanage-agent', False)
            run(prefix + ['disable', '--now', UNIT])
            if run(prefix + ['show', UNIT, '-p', 'ActiveState', '--value']) not in ('inactive', 'failed') or run(prefix + ['show', UNIT, '-p', 'MainPID', '--value']) != '0':
                raise RuntimeError('Basic Agent still running')
            if run(prefix + ['is-enabled', UNIT], expected=(0, 1, 3, 4)) not in ('disabled', 'masked', 'masked-runtime', 'static', 'indirect'):
                raise RuntimeError('Basic autostart still enabled')
        elif backend == 'systemd':
            raise RuntimeError('User service manager unavailable')
        elif not launcher.exists():
            raise RuntimeError('Compatibility controller unavailable')
    else:
        raise RuntimeError('Invalid privilege mode')
    print('AGENT_STOPPED_FILES_RETAINED', flush=True)


if __name__ == '__main__':
    try:
        if len(sys.argv) != 5:
            raise RuntimeError('Invalid fixed operation')
        stop(*sys.argv[1:])
    except (OSError, ValueError, RuntimeError, sqlite3.Error, subprocess.TimeoutExpired):
        print('Agent shutdown not confirmed; installation and identity retained.', file=sys.stderr)
        raise SystemExit(1)
