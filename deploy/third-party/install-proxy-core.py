#!/usr/bin/env python3
"""Explicit per-user Linux x86_64 probe core install; no service or routing changes."""
import argparse,gzip,hashlib,io,json,os,platform,stat,tempfile,urllib.request
from pathlib import Path

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply',action='store_true')
    parser.add_argument('--archive',type=Path,help='Use a previously downloaded archive; the same pinned checksum is mandatory')
    args=parser.parse_args()
    pin=json.loads(Path(__file__).with_name('mihomo.json').read_text())
    directory=Path.home()/'.easymanage/bin';target=directory/'mihomo-probe'
    if platform.system()!='Linux' or platform.machine() not in ('x86_64','amd64'):raise SystemExit('This pinned artifact supports Linux x86_64 only')
    if not args.apply:
        print(f"Would install {pin['tag']} into {target}; rerun with --apply")
        return
    directory.mkdir(parents=True,exist_ok=True,mode=0o700)
    m=directory.lstat()
    if not stat.S_ISDIR(m.st_mode) or m.st_uid!=os.getuid() or m.st_mode&0o022:raise SystemExit('Unsafe binary directory; check owner and write permissions')
    if target.exists() or target.is_symlink():raise SystemExit('Existing probe core retained; move it to a private backup before an explicit upgrade')
    if args.archive:
        with args.archive.open('rb') as source:archive=source.read(32*1024*1024+1)
    else:
        opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open(pin['linux_amd64_asset'],timeout=30) as response:archive=response.read(32*1024*1024+1)
    if len(archive)>32*1024*1024 or hashlib.sha256(archive).hexdigest()!=pin['archive_sha256']:raise SystemExit('Pinned release archive checksum mismatch')
    with gzip.GzipFile(fileobj=io.BytesIO(archive)) as f:binary=f.read(64*1024*1024+1)
    if len(binary)>64*1024*1024 or not binary.startswith(b'\x7fELF'):raise SystemExit('Invalid or oversized core binary')
    fd,name=tempfile.mkstemp(prefix='.mihomo-probe-',dir=directory)
    try:
        with os.fdopen(fd,'wb') as f:f.write(binary);f.flush();os.fsync(f.fileno());os.fchmod(f.fileno(),0o700)
        os.link(name,target)
    finally:Path(name).unlink(missing_ok=True)
    print(f"Installed {pin['tag']} probe executable; no service was started or reloaded")

if __name__=='__main__':main()
