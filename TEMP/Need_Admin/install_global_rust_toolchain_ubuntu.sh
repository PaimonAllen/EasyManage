#!/usr/bin/env bash
set -Eeuo pipefail

readonly rust_version="1.98.1"
readonly rust_target="x86_64-unknown-linux-gnu"
readonly rust_archive="rust-${rust_version}-${rust_target}.tar.xz"
readonly rust_base_url="https://static.rust-lang.org/dist"

if [[ ! -r /etc/os-release ]]; then
  printf 'Cannot identify this operating system.\n' >&2
  exit 1
fi

# shellcheck disable=SC1091
source /etc/os-release
if [[ "${ID:-}" != "ubuntu" ]]; then
  printf 'This script supports Ubuntu only; detected: %s\n' "${ID:-unknown}" >&2
  exit 1
fi

if [[ "$(uname -m)" != "x86_64" ]]; then
  printf 'This script currently supports x86_64 only; detected: %s\n' "$(uname -m)" >&2
  exit 1
fi

if (( EUID != 0 )); then
  if ! command -v sudo >/dev/null 2>&1; then
    printf 'Administrator privileges are required and sudo is unavailable.\n' >&2
    exit 1
  fi
  exec sudo bash "$0" "$@"
fi

if [[ -x /usr/local/bin/rustc ]] \
  && /usr/local/bin/rustc --version | grep --quiet "rustc ${rust_version} "; then
  printf 'Global Rust %s is already installed in /usr/local.\n' "$rust_version"
  exit 0
fi

temporary_dir="$(mktemp --directory --tmpdir rust-global-install.XXXXXXXXXX)"
cleanup() {
  find "$temporary_dir" -depth -delete
}
trap cleanup EXIT

curl --proto '=https' --tlsv1.2 --fail --location --show-error \
  --output "$temporary_dir/$rust_archive" \
  "$rust_base_url/$rust_archive"
curl --proto '=https' --tlsv1.2 --fail --location --show-error \
  --output "$temporary_dir/$rust_archive.sha256" \
  "$rust_base_url/$rust_archive.sha256"

(
  cd "$temporary_dir"
  sha256sum --check "$rust_archive.sha256"
)

tar --extract --file "$temporary_dir/$rust_archive" --directory "$temporary_dir"
"$temporary_dir/rust-${rust_version}-${rust_target}/install.sh" \
  --prefix=/usr/local \
  --disable-ldconfig

/usr/local/bin/rustc --version
/usr/local/bin/cargo --version
/usr/local/bin/rustfmt --version
/usr/local/bin/cargo-clippy --version
