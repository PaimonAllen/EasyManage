#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
set -Eeuo pipefail

bundle_directory="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if (( EUID != 0 )); then
  printf 'Run this staged installer as root, for example:\n' >&2
  printf '  sudo bash %q\n' "$bundle_directory/INSTALL_AS_ADMIN.sh" >&2
  exit 1
fi
if [[ ! "$bundle_directory" =~ ^/tmp/easymanage-agent\.[A-Za-z0-9]+$ ]]; then
  printf 'Unsafe bundle location; expected /tmp/easymanage-agent.RANDOM.\n' >&2
  exit 1
fi

cd "$bundle_directory"
sha256sum --check SHA256SUMS
bash "$bundle_directory/install-agent.sh" \
  "$bundle_directory/easymanage-agent" \
  "$bundle_directory/ca.crt" \
  "$bundle_directory/agent.env" \
  "$bundle_directory/easymanage-agent.service"
systemctl is-active --quiet easymanage-agent.service

# The enrollment token is no longer needed in /tmp after the root-owned copy is installed.
find "$bundle_directory" -mindepth 1 -maxdepth 1 -type f -delete
rmdir "$bundle_directory"

printf '\nEasyManage agent is active; the staging bundle has been removed.\n'
