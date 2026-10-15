#!/usr/bin/env bash
set -Eeuo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd -- "$script_dir/.." && pwd)"
cd "$project_root"
binary_path="$project_root/target/release/easymanage-agent"
[[ -x "$binary_path" ]] || { printf 'Run ./scripts/run-server.sh --build first.\n' >&2; exit 1; }
user_env="${HOME:?}/.local/share/easymanage/config/agent.env"
if [[ -r "$user_env" ]]; then export EASYMANAGE_ENV_FILE="${EASYMANAGE_ENV_FILE:-$user_env}"; fi
exec "$binary_path"
