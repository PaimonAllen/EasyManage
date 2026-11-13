#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mode=release
publish=false
for arg in "$@"; do case "$arg" in --check) mode=check;; --release) mode=release;; --publish) publish=true;; *) echo "Usage: $0 [--check|--release] [--publish]" >&2; exit 2;; esac; done
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
triple=x86_64-pc-windows-gnu
private="$PWD/third_party/windows-build/root"
if command -v x86_64-w64-mingw32-gcc >/dev/null; then
    export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
elif [[ -x "$private/usr/bin/x86_64-w64-mingw32-gcc-posix" ]]; then
    export PATH="$private/usr/bin:$PATH"
    wrapper="$PWD/third_party/windows-build/mingw-cc"
    cat > "$wrapper" <<EOF
#!/usr/bin/env bash
export GCC_EXEC_PREFIX="$private/usr/lib/gcc/"
export LIBRARY_PATH="$private/usr/x86_64-w64-mingw32/lib"
exec "$private/usr/bin/x86_64-w64-mingw32-gcc-posix" -B"$private/usr/bin/" --sysroot="$private" -I"$private/usr/x86_64-w64-mingw32/include" -L"$private/usr/x86_64-w64-mingw32/lib" "\$@"
EOF
    chmod 700 "$wrapper"
    export CC_x86_64_pc_windows_gnu="$wrapper"
    export CFLAGS_x86_64_pc_windows_gnu="--sysroot=$private -I$private/usr/x86_64-w64-mingw32/include"
else
    echo 'Install/use an x86_64 MinGW compiler, or extract the compiler into third_party/windows-build/root (no production package changes required).' >&2; exit 1
fi
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$CC_x86_64_pc_windows_gnu"
if [[ "$mode" == check ]]; then nice -n 10 cargo check -p easymanage-agent --target "$triple"; else nice -n 10 cargo build -p easymanage-agent --release --target "$triple"; fi
if $publish; then
    [[ "$mode" == release ]] || { echo '--publish requires --release' >&2; exit 2; }
    python3 - <<'PY'
import os, pathlib, shutil, stat
root=pathlib.Path(os.environ.get('EASYMANAGE_RELEASE_ROOT',str(pathlib.Path.home()/'.easymanage/releases/current'))).resolve(strict=True)
for name in ['target/x86_64-pc-windows-gnu/release/easymanage-agent.exe','deploy/windows/agent-control.ps1']:
    destination=root/name
    destination.parent.mkdir(parents=True,exist_ok=True)
    if destination.is_symlink(): raise RuntimeError('Refusing linked candidate')
    tmp=destination.with_name(destination.name+'.next')
    with open(tmp,'xb') as output: output.write(pathlib.Path(name).read_bytes())
    tmp.chmod(0o700 if destination.suffix=='.exe' else 0o600);tmp.replace(destination)
print('Windows candidate published. No running service was updated or restarted.')
PY
fi
