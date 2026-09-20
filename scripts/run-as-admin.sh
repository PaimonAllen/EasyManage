#!/usr/bin/env bash
set -Eeuo pipefail

if (( $# != 1 )); then
  printf 'Usage: %s <server|agent>\n' "$0" >&2
  exit 64
fi

case "$1" in
  server)
    package_name="easymanage-server"
    ;;
  agent)
    package_name="easymanage-agent"
    ;;
  *)
    printf 'Unknown component: %s (expected server or agent)\n' "$1" >&2
    exit 64
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd -- "$script_dir/.." && pwd)"
binary_path="$project_root/target/debug/$package_name"
admin_policy="${EASYMANAGE_ADMIN_POLICY:-warn}"

cd "$project_root"
cargo build --package "$package_name"

if (( EUID == 0 )); then
  exec env EASYMANAGE_ADMIN_POLICY="$admin_policy" "$binary_path"
fi

if command -v sudo >/dev/null 2>&1; then
  if sudo --non-interactive true 2>/dev/null; then
    exec sudo env EASYMANAGE_ADMIN_POLICY="$admin_policy" "$binary_path"
  fi

  if [[ -t 0 && -t 1 ]] && sudo --validate; then
    exec sudo env EASYMANAGE_ADMIN_POLICY="$admin_policy" "$binary_path"
  fi
fi

printf '[ERROR] Could not obtain administrator privileges for %s; continuing as the current user.\n' \
  "$package_name" >&2
exec env EASYMANAGE_ADMIN_POLICY="$admin_policy" "$binary_path"

