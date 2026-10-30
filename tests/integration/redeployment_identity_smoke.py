#!/usr/bin/env python3
"""Same-identity admission checks; require disposable network/mount/home masks."""
import os
from pathlib import Path
import runpy
import sqlite3
import subprocess
import sys
import uuid
from unittest.mock import patch

assert len(sys.argv) == 3
for kind, original in zip(("net", "mnt"), sys.argv[1:]):
    assert os.readlink(f"/proc/self/ns/{kind}") != original
mounts = Path('/proc/self/mountinfo').read_text().splitlines()
assert any(line.split()[4] == '/home' and ' - tmpfs ' in line for line in mounts)
assert os.geteuid() != 0
os.umask(0o077)
base = Path.home() / '.easymanage'
(base/'state').mkdir(parents=True)
(base/'config').mkdir()
database = base/'state/state.db'
node = str(uuid.uuid4())
with sqlite3.connect(database) as db:
    db.execute('CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL)')
    db.execute('INSERT INTO metadata VALUES(?,?)',('node_id', node))
env = base/'config/agent.env'
env.write_text('EASYMANAGE_NODE_ID='+node+'\nEASYMANAGE_AGENT_STATE_DB='+str(database)+'\n')
check = runpy.run_path('deploy/linux/operations/check-redeployment.py')['check']


def refused(action):
    try:
        action()
    except (ValueError, sqlite3.Error):
        return
    raise AssertionError('Unsafe repair was accepted')


with patch('subprocess.run', return_value=subprocess.CompletedProcess([],0,stdout=b'not-found\n')):
    assert check(str(base), node, 'basic') == {'same_identity': True}
    refused(lambda:check(str(base), str(uuid.uuid4()), 'basic'))
    original = database.read_bytes()
    env.write_text('EASYMANAGE_NODE_ID='+str(uuid.uuid4())+'\n')
    refused(lambda:check(str(base), node, 'basic'))
    assert database.read_bytes() == original
    env.write_text('EASYMANAGE_NODE_ID='+node+'\nEASYMANAGE_AGENT_STATE_DB=/tmp/abandoned.db\n')
    refused(lambda:check(str(base), node, 'basic'))
    env.write_text('EASYMANAGE_NODE_ID='+node+'\n')
    env.chmod(0o666)
    refused(lambda:check(str(base), node, 'basic'))
    env.chmod(0o600)
    retained = database.with_suffix('.original')
    database.rename(retained)
    database.symlink_to(retained)
    refused(lambda:check(str(base), node, 'basic'))
    database.unlink()
    retained.rename(database)
    with patch('subprocess.run', return_value=subprocess.CompletedProcess([],0,stdout=b'loaded\n')):
        refused(lambda:check(str(base), node, 'basic'))
    with patch('shutil.which', return_value=None):
        assert check(str(base), node, 'basic')['same_identity']
print('PASS isolated redeployment identity: same UUID, wrong UUID/config, custom state path, ownership/modes, symlink and Full conflict checks; no state resets')
