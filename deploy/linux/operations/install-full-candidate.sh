#!/usr/bin/env bash
# Stage a verified release in an ordinary user's private agent directory.
# The running Full service is changed only by promote-agent.sh after sudo auth.
set -Eeuo pipefail
umask 0077
(( EUID != 0 )) || exit 64
bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
[[ "$bundle_dir" =~ ^/tmp/easymanage-agent\.[a-zA-Z0-9_-]+$ ]] || exit 64
account="$(id -un)"
account_home="$(getent passwd "$account" | cut -d: -f6)"
agent_base="$account_home/.local/share/easymanage"
[[ "$account" =~ ^[a-z_][a-z0-9_-]*$ && "$account_home" == "/home/$account" ]] || exit 64
for path in "$bundle_dir/easymanage-agent" "$bundle_dir/SHA256SUMS" "$agent_base/bin/easymanage-agent"; do
  cursor="$path"
  while [[ "$cursor" != / ]]; do
    [[ ! -L "$cursor" ]] || exit 64
    cursor="$(dirname -- "$cursor")"
  done
done
[[ -f "$agent_base/config/agent.env" && -f "$agent_base/config/ca.crt" ]] || exit 64
[[ -f "$bundle_dir/easymanage-agent" && -f "$bundle_dir/SHA256SUMS" ]] || exit 64
(cd "$bundle_dir" && sha256sum --check --status SHA256SUMS)
[[ "$(systemctl show -p ActiveState --value easymanage-agent.service)" == active ]] || exit 3
[[ ! -e "$agent_base/bin/easymanage-agent" || -f "$agent_base/bin/easymanage-agent" ]] || exit 64
backup_dir="$(mktemp -d "$agent_base/full-candidate.XXXXXX")"
if [[ -f "$agent_base/bin/easymanage-agent" ]]; then
  cp -p -- "$agent_base/bin/easymanage-agent" "$backup_dir/previous-agent"
fi
temporary="$(mktemp "$agent_base/bin/.easymanage-candidate.XXXXXX")"
trap 'unlink "$temporary" 2>/dev/null || true' EXIT
install -m 0700 "$bundle_dir/easymanage-agent" "$temporary"
mv -T -- "$temporary" "$agent_base/bin/easymanage-agent"
trap - EXIT
printf 'Full candidate staged as %s; current Full service is unchanged. Recovery: %s\n' "$account" "$backup_dir"
