#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
set -Eeuo pipefail

show_help() {
  printf '%s\n' \
    'Install or update the EasyManage agent from already staged files.' \
    '' \
    "Usage: $0 BINARY CA_CERTIFICATE AGENT_ENV SERVICE_UNIT"
}

if (( $# != 4 )); then
  show_help >&2
  exit 64
fi

binary_source="$1"
ca_source="$2"
environment_source="$3"
unit_source="$4"

if (( EUID != 0 )); then
  if ! command -v sudo >/dev/null 2>&1; then
    printf 'Administrator privileges are required and sudo is unavailable.\n' >&2
    exit 1
  fi
  exec sudo bash "$0" "$@"
fi

for required in "$binary_source" "$ca_source" "$environment_source" "$unit_source"; do
  if [[ ! -f "$required" ]]; then
    printf 'Required staged file is missing: %s\n' "$required" >&2
    exit 1
  fi
done
for variable_name in EASYMANAGE_SERVER_URL EASYMANAGE_CA_CERTIFICATE EASYMANAGE_AGENT_STATE_DB EASYMANAGE_ENROLLMENT_TOKEN; do
  if ! grep -q "^${variable_name}=" "$environment_source"; then
    printf '%s is missing from the staged agent environment.\n' "$variable_name" >&2
    exit 1
  fi
done

install -d -o root -g root -m 0700 /etc/easymanage /etc/easymanage/tls /var/lib/easymanage-agent
install -o root -g root -m 0755 "$binary_source" /usr/local/sbin/easymanage-agent
install -o root -g root -m 0644 "$ca_source" /etc/easymanage/tls/ca.crt
install -o root -g root -m 0600 "$environment_source" /etc/easymanage/agent.env
install -o root -g root -m 0644 "$unit_source" /etc/systemd/system/easymanage-agent.service

systemctl daemon-reload
systemctl enable easymanage-agent.service
systemctl restart easymanage-agent.service
systemctl --no-pager --full status easymanage-agent.service || true

printf '\nEasyManage agent installation completed.\n'
