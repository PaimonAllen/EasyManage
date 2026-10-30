#!/usr/bin/env python3
"""Read-only scanner behavior in private namespaces and synthetic files only."""
import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
from unittest.mock import patch

assert len(sys.argv) == 3
for kind, original in zip(('net', 'mnt'), sys.argv[1:]):
    assert os.readlink('/proc/self/ns/'+kind) != original
assert any(line.split()[4] == '/tmp' and ' - tmpfs ' in line for line in Path('/proc/self/mountinfo').read_text().splitlines())
module = runpy.run_path('deploy/linux/operations/scan-disk-space.py')
scan = module['scan']
with tempfile.TemporaryDirectory(dir='/tmp') as temporary:
    root = Path(temporary)
    (root/'alpha').mkdir()
    (root/'alpha'/'real.bin').write_bytes(b'fixture-content'*1024)
    os.link(root/'alpha'/'real.bin', root/'hardlink')
    with (root/'sparse').open('wb') as stream:
        stream.truncate(8*1024*1024)
    (root/'link').symlink_to('/etc')
    (root/'empty').mkdir()
    os.mkfifo(root/'pipe')
    (root/'odd;$(touch nope)').write_text('fixture')
    before=hashlib.sha256((root/'alpha'/'real.bin').read_bytes()).hexdigest()
    def run(**kwargs):
        return scan(str(root), allowed_fs={'tmpfs'}, **kwargs)
    result=run()
    assert result['status']=='complete',result
    nodes=result['nodes'];by_name={node['name']:node for node in nodes}
    assert result['hardlinks']==1 and result['symlinks']==1
    assert set(by_name)=={str(root),'alpha','empty'}
    assert all(n['kind']=='directory' for n in nodes)
    # File names are transient, but all byte/file totals include them. Hard-link
    # placement can differ by enumeration order, so verify the whole subtree.
    stats=[root.stat(),(root/'alpha').stat(),(root/'empty').stat(),
           (root/'alpha/real.bin').stat(),(root/'sparse').stat(),
           (root/'link').lstat(),(root/'odd;$(touch nope)').stat()]
    assert nodes[0]['apparent']==sum(m.st_size for m in stats)
    assert nodes[0]['allocated']==sum(m.st_blocks*512 for m in stats)
    assert nodes[0]['files']==4 and result['saved_directories']==3
    assert result['detail_level']=='directories'
    assert before==hashlib.sha256((root/'alpha'/'real.bin').read_bytes()).hexdigest()
    batches=[]
    streamed=run(stream=True,emit=batches.append)
    merged={n['id']:n for batch in batches if batch['type']=='entries' for n in batch['nodes']}
    assert sorted(merged.values(),key=lambda n:n['id'])==nodes and streamed['nodes']==[nodes[0]]
    assert streamed['hardlinks']==1 and streamed['paged']
    assert all(n['kind']=='directory' for n in merged.values())
    assert not any(json.dumps(secret) in json.dumps(batches) for secret in ['real.bin','hardlink','sparse','odd;$(touch nope)'])
    assert not list(Path('/tmp').glob('easymanage-disk-links-*'))
    with patch('time.monotonic',side_effect=range(1000000)),patch('time.sleep'):
        timed=run(seconds=5)
    assert timed['status']=='partial' and timed['reason']=='time_limit'
    limited=run(max_nodes=3)
    assert limited['status']=='partial' and limited['reason']=='entry_limit' and limited['nodes'][0]['incomplete']
    assert run(max_depth=1)['status']=='partial'
    assert run(cancelled=lambda:True)['status']=='cancelled'
    real_open=os.open
    def denied(path, *args, **kwargs):
        if path=='alpha': raise PermissionError(13,'fixture denied')
        return real_open(path,*args,**kwargs)
    with patch('os.open',denied):
        denied_result=run()
    assert denied_result['status']=='partial' and denied_result['errors']>0
    assert next(n for n in denied_result['nodes'] if n['name']=='alpha')['issue']=='permission_denied'
    for invalid in ['relative',str(root)+'/../etc',str(root)+'/link',str(root)+'/link/ssl']:
        try: scan(invalid,allowed_fs={'tmpfs'})
        except (ValueError,OSError):pass
        else:raise AssertionError('Unsafe path accepted')
    try: scan(str(root))
    except ValueError:pass
    else:raise AssertionError('Virtual filesystem accepted by production default')
    # A replaced directory must not be followed after enumeration.
    def replaced(path,*args,**kwargs):
        if path=='alpha':
            (root/'alpha').rename(root/'original-alpha')
            (root/'alpha').symlink_to('/etc')
        return real_open(path,*args,**kwargs)
    with patch('os.open',replaced): raced=run()
    assert raced['status']=='partial' and not any(n['name']=='passwd' for n in raced['nodes'])
    print('PASS allocated/apparent totals, sparse files, hardlinks, symlinks/FIFO, unusual names, permission errors, cancellation, entry/depth limits and symlink replacement; content unchanged')

# Visit more than the old file limit without persisting any file metadata.
with tempfile.TemporaryDirectory(dir='/tmp') as temporary:
    root=Path(temporary)
    for index in range(32010):
        (root/(str(index).zfill(6)+'x'*230)).touch()
    count=wire_bytes=0
    def receive(value):
        global count,wire_bytes
        wire_bytes+=len(json.dumps(value).encode())
        if value['type']=='entries':
            assert 1<=len(value['nodes'])<=256
            for node in value['nodes']:
                if node['id']==count:count+=1
                else:assert node['id']==0  # final root totals
    # Advance the clock well past all previous deadlines, without waiting hours.
    with patch('time.monotonic',side_effect=range(1000000)),patch('time.sleep'):
        result=scan(str(root),seconds=0,max_nodes=0,stream=True,emit=receive,allowed_fs={'tmpfs'})
    assert result['status']=='complete' and result['visited']==32011 and count==1
    assert result['elapsed_seconds']>140 and wire_bytes<8*1024*1024
    assert result['saved_directories']==1
    assert len(result['nodes'])==1 and result['nodes'][0]['files']==32010
    assert not list(Path('/tmp').glob('easymanage-disk-links-*'))
    print('PASS unlimited traversal beyond 32,000 files and former deadlines; only one directory persisted, no temporary database')

# Nested private mount namespace proves same-device bind mounts are boundaries.
code='''import os,runpy,pathlib,subprocess,tempfile,json
m=runpy.run_path('/tmp/work/deploy/linux/operations/scan-disk-space.py')
root=pathlib.Path(tempfile.mkdtemp(dir='/tmp'));(root/'tree').mkdir();(root/'tree/other').mkdir();(root/'source').mkdir();(root/'source/hidden').write_text('fixture')
subprocess.run(['mount','--bind',str(root/'source'),str(root/'tree/other')],check=True)
r=m['scan'](str(root/'tree'),allowed_fs={'tmpfs'})
assert r['skipped_mounts']==1 and not any(n['name']=='hidden' for n in r['nodes'])
assert next(n for n in r['nodes'] if n['name']=='other')['issue']=='mount_boundary'
print('PASS same-device bind mount skipped without following it')
# Actual separate filesystems plus two overlapping bind aliases; all synthetic.
from unittest.mock import patch
a=root/'a';b=root/'b';a.mkdir();b.mkdir()
for disk in (a,b):subprocess.run(['mount','-t','tmpfs','-o','size=8m','tmpfs',str(disk)],check=True)
(a/'root-file').write_bytes(b'a'*100)
(b/'user').mkdir();(b/'user/payload').write_bytes(b'b'*4096)
for alias,source in [('bigdata',b/'user'),('mirror',b)]:
 (a/alias).mkdir();subprocess.run(['mount','--bind',str(source),str(a/alias)],check=True)
original=m['mounts']
table=[e for e in original() if e['path'] in {str(a),str(b),str(a/'bigdata'),str(a/'mirror')}]
globals=m['scan'].__globals__
globals['mounts']=lambda:table
batches=[]
r=m['scan']('/',scope='all_local',allowed_fs={'tmpfs'},stream=True,emit=batches.append)
entries={n['id']:n for batch in batches if batch['type']=='entries' for n in batch['nodes']}
assert r['status']=='complete' and r['scope']=='all_local'
assert {n['scan_path'] for n in entries.values() if n['scan_path']}=={str(a),str(b)}
assert not any(n['name']=='payload' for n in entries.values())
assert next(n for n in entries.values() if n['name']=='user')['files']==1
assert r['nodes'][0]['files']==2
assert sum(n['issue']=='duplicate_mount' for n in entries.values())==2
assert r['nodes'][0]['allocated']==sum(n['allocated'] for n in entries.values() if n['parent']==0)
assert all(n['incomplete'] is False for n in entries.values())
assert [n['id'] for n in entries.values()]==list(range(r['saved_directories']))
limited=m['scan']('/',scope='all_local',allowed_fs={'tmpfs'},max_nodes=3)
assert limited['status']=='partial' and limited['reason']=='entry_limit'
assert m['scan']('/',scope='all_local',allowed_fs={'tmpfs'},cancelled=lambda:True)['status']=='cancelled'
# If the canonical root is unreadable, a readable alias still contributes data.
real_open=globals['open_directory']
def denied(path):
 if path==str(b):raise PermissionError(13,'fixture')
 return real_open(path)
with patch.dict(globals,open_directory=denied):fallback=m['scan']('/',scope='all_local',allowed_fs={'tmpfs'})
assert fallback['status']=='partial' and fallback['nodes'][0]['files']==2
assert not any(n['name']=='payload' for n in fallback['nodes'])
# Network filesystems are not selected even if present in the mount snapshot.
with patch.dict(globals,mounts=lambda:[e|{'filesystem':'nfs4'} for e in table]):
 excluded=m['scan']('/',scope='all_local',allowed_fs={'tmpfs'})
assert excluded['status']=='partial' and excluded['nodes'][0]['files']==0
globals['mounts']=original
print('PASS all-local filesystem coverage, bind deduplication, original paths, global limits/cancel, unreadable-root alias fallback and network exclusion')
script=pathlib.Path('/tmp/work/deploy/linux/operations/scan-disk-space.py').read_text()
request={'path':'/tmp/work/tests/integration/fixtures','mode':'full','seconds':10,'username':'fixture'}
p=subprocess.Popen(['python3','-I','-B','-c',script,json.dumps(request)],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
while True:
 line=p.stdout.readline()
 if not line:break
 value=json.loads(line)
 if value.get('type')=='result':break
p.stdin.close();assert p.wait(timeout=5)==0
assert value['uid']==0 and value['status']=='complete'
p=subprocess.run(['python3','-I','-B','-c',script,json.dumps(request|{'mode':'basic'})],capture_output=True)
assert p.returncode!=0 and b'"type":"error"' in p.stdout
print('PASS actual namespace-root scanner, Full identity and Basic/root mismatch refusal')
'''
subprocess.run(['bwrap','--unshare-user','--uid','0','--gid','0','--cap-add','CAP_SYS_ADMIN','--unshare-pid','--ro-bind','/','/',
                '--tmpfs','/tmp','--proc','/proc','--dev','/dev','--dir','/tmp/work','--ro-bind',os.getcwd(),'/tmp/work',
                'python3','-B','-c',code],check=True)
