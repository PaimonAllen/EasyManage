#!/usr/bin/env bash
set -Eeuo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd -- "$script_dir/.." && pwd)"
cd "$project_root"
case "${1:-}" in
  --publish-platform)
    exec python3 "$project_root/deploy/linux/operations/platform-release.py" publish --source "$project_root"
    ;;
  --fleet|--prepare-fleet|--publish-agent|--controller)
    action=deploy
    [[ "$1" != --prepare-fleet ]] || action=prepare
    [[ "$1" != --publish-agent ]] || action=publish
    [[ "$1" != --controller ]] || action=controller
    shift
    exec python3 "$project_root/deploy/orchestration/fleet.py" "$action" "$@"
    ;;
  --start-all|--status)
    action=start
    [[ "$1" != --status ]] || action=status
    shift
    exec python3 "$project_root/deploy/orchestration/fleet.py" "$action" "$@"
    ;;
  --build)
    env -u CFLAGS -u CPPFLAGS -u CXXFLAGS -u LDFLAGS -u PKG_CONFIG_PATH \
      CC=/usr/bin/cc CXX=/usr/bin/c++ AR=/usr/bin/ar \
      nice -n 10 cargo build --workspace --release --jobs 2
    pnpm --dir "$project_root/web" install --frozen-lockfile
    pnpm --dir "$project_root/web" build
    pnpm --dir "$project_root/docs" install --frozen-lockfile --ignore-scripts
    pnpm --dir "$project_root/docs" build
    if [[ -f "${HOME:?}/.easymanage/config/targets.json" ]]; then
      if ! python3 "$project_root/deploy/linux/operations/platform-release.py" publish --source "$project_root"; then
        printf '%s\n' 'Platform candidate was not published; check owner/ancestor permissions. Agent publication continues.' >&2
      fi
      python3 "$project_root/deploy/orchestration/fleet.py" publish
    fi
    exit 0
    ;;
  --help|-h)
    printf '%s\n' 'run-server.sh [--build|--controller|--publish-platform|--publish-agent|--fleet|--prepare-fleet|--start-all|--status]'
    printf '%s\n' 'No arguments: foreground current-user execution. --fleet/--controller/--prepare-fleet/--start-all manage Linux user services; Full Agents are retained.'
    exit 0
    ;;
  '') ;;
  *) printf 'Unknown option: %s\n' "$1" >&2; exit 64 ;;
esac
binary_path="$project_root/target/release/easymanage-server"
[[ -x "$binary_path" ]] || { printf 'Run ./scripts/run-server.sh --build first.\n' >&2; exit 1; }
user_env="${HOME:?}/.easymanage/config/server.env"
if [[ -r "$user_env" ]]; then export EASYMANAGE_ENV_FILE="${EASYMANAGE_ENV_FILE:-$user_env}"; fi
exec "$binary_path"
