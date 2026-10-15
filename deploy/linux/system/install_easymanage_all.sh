#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
set -Eeuo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd -- "$script_dir/../../.." && pwd)"
remote_target="${EASYMANAGE_REMOTE_TARGET:-}"
server_address="${EASYMANAGE_SERVER_ADDRESS:-}"
local_stage=""
remote_stage=""
keep_remote_stage="false"
remote_install_mode="automatic"

case "${1:-}" in
  '') ;;
  --remote-manual) remote_install_mode="manual" ;;
  -h|--help)
    printf '%s\n' \
      "Usage: $0 [--remote-manual]" \
      '' \
      '  --remote-manual  Stage the server 2 bundle in /tmp without invoking remote sudo.'
    exit 0
    ;;
  *)
    printf 'Unknown option: %s\n' "$1" >&2
    exit 64
    ;;
esac
if (( $# > 1 )); then
  printf 'Only one option is supported.\n' >&2
  exit 64
fi

[[ -n "$remote_target" && -n "$server_address" ]] || {
  printf 'Set EASYMANAGE_REMOTE_TARGET and EASYMANAGE_SERVER_ADDRESS explicitly; there are no production defaults.\n' >&2
  exit 64
}

if (( EUID == 0 )); then
  if [[ -n "${EASYMANAGE_OPERATOR_USER:-}" ]]; then
    operator_user="$EASYMANAGE_OPERATOR_USER"
  else
    operator_user="$(stat -c %U "$project_root")"
    if [[ -z "$operator_user" || "$operator_user" == "root" ]]; then
      operator_user="${SUDO_USER:-root}"
    fi
  fi
else
  operator_user="$(id -un)"
fi
operator_uid="$(id -u "$operator_user")"
operator_gid="$(id -g "$operator_user")"
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
if [[ -z "$operator_home" || ! -d "$operator_home" ]]; then
  printf 'Home directory for SSH operator %s is unavailable.\n' "$operator_user" >&2
  exit 1
fi
ssh_config_options=()
operator_ssh_config="${EASYMANAGE_REMOTE_SSH_CONFIG:-$operator_home/.ssh/config}"
if [[ -f "$operator_ssh_config" ]]; then
  ssh_config_options=(-F "$operator_ssh_config")
fi
if [[ -n "${EASYMANAGE_REMOTE_IDENTITY_FILE:-}" ]]; then
  if [[ ! -f "$EASYMANAGE_REMOTE_IDENTITY_FILE" ]]; then
    printf 'SSH identity file does not exist: %s\n' "$EASYMANAGE_REMOTE_IDENTITY_FILE" >&2
    exit 1
  fi
  ssh_config_options+=(-i "$EASYMANAGE_REMOTE_IDENTITY_FILE" -o IdentitiesOnly=yes)
fi

run_as_operator() {
  if (( EUID == 0 )); then
    runuser -u "$operator_user" -- \
      env HOME="$operator_home" USER="$operator_user" LOGNAME="$operator_user" "$@"
  else
    "$@"
  fi
}

run_as_administrator() {
  if (( EUID == 0 )); then
    "$@"
  else
    sudo "$@"
  fi
}

cleanup() {
  if [[ -n "$local_stage" && "$local_stage" == /tmp/easymanage-deploy.* && -d "$local_stage" ]]; then
    find "$local_stage" -mindepth 1 -maxdepth 1 -delete
    rmdir "$local_stage"
  fi
  if [[ "$keep_remote_stage" != "true" && -n "$remote_stage" && "$remote_stage" =~ ^/tmp/easymanage-agent\.[A-Za-z0-9]+$ ]]; then
    run_as_operator ssh "${ssh_config_options[@]}" -o BatchMode=yes "$remote_target" \
      "find '$remote_stage' -mindepth 1 -maxdepth 1 -delete && rmdir '$remote_stage'" \
      >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if [[ ! "$server_address" =~ ^[0-9a-fA-F:.]+$ ]]; then
  printf 'EASYMANAGE_SERVER_ADDRESS must be an IP address.\n' >&2
  exit 1
fi
for required in \
  "$project_root/target/release/easymanage-server" \
  "$project_root/target/release/easymanage-agent" \
  "$project_root/web/dist/index.html"; do
  if [[ ! -e "$required" ]]; then
    printf 'Required release artifact is missing: %s\n' "$required" >&2
    exit 1
  fi
done

resolved_ssh="$({
  run_as_operator ssh "${ssh_config_options[@]}" -G "$remote_target" 2>/dev/null \
    | awk '$1 == "hostname" || $1 == "user" || $1 == "port" {print $1 "=" $2}'
} || true)"
printf 'Remote SSH preflight: operator=%s config=%s target=%s %s\n' \
  "$operator_user" \
  "${operator_ssh_config:-default}" \
  "$remote_target" \
  "$(tr '\n' ' ' <<< "$resolved_ssh")"
if ! run_as_operator ssh "${ssh_config_options[@]}" \
  -o BatchMode=yes -o ConnectTimeout=5 "$remote_target" true; then
  printf 'Remote SSH preflight failed before installation.\n' >&2
  printf 'Set EASYMANAGE_OPERATOR_USER and EASYMANAGE_REMOTE_SSH_CONFIG explicitly if needed.\n' >&2
  exit 1
fi

EASYMANAGE_SERVER_ADDRESS="$server_address" \
  bash "$script_dir/install_easymanage_server_and_agent.sh"

local_stage="$(run_as_operator mktemp -d /tmp/easymanage-deploy.XXXXXX)"
run_as_administrator install -o "$operator_uid" -g "$operator_gid" -m 0600 \
  /etc/easymanage/tls/ca.crt "$local_stage/ca.crt"
enrollment_token="$(
  run_as_administrator sed -n 's/^EASYMANAGE_ENROLLMENT_TOKEN=//p' /etc/easymanage/server.env
)"
if [[ ${#enrollment_token} -lt 32 ]]; then
  printf 'Could not read a valid enrollment token from server.env.\n' >&2
  exit 1
fi

umask 0077
printf '%s\n' \
  "EASYMANAGE_SERVER_URL=https://${server_address}:8443" \
  'EASYMANAGE_CA_CERTIFICATE=/etc/easymanage/tls/ca.crt' \
  'EASYMANAGE_AGENT_STATE_DB=/var/lib/easymanage-agent/state.db' \
  "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' \
  > "$local_stage/agent.env"
install -m 0755 "$project_root/target/release/easymanage-agent" "$local_stage/easymanage-agent"
install -m 0755 "$script_dir/install_easymanage_agent.sh" "$local_stage/install-agent.sh"
install -m 0755 "$script_dir/install_staged_easymanage_agent.sh" \
  "$local_stage/INSTALL_AS_ADMIN.sh"
install -m 0644 "$project_root/deploy/linux/system/easymanage-agent.service" "$local_stage/easymanage-agent.service"
(
  cd "$local_stage"
  sha256sum \
    easymanage-agent \
    ca.crt \
    agent.env \
    install-agent.sh \
    INSTALL_AS_ADMIN.sh \
    easymanage-agent.service \
    > SHA256SUMS
)
run_as_administrator chown -R "$operator_uid:$operator_gid" "$local_stage"

server_ready="false"
for _ in {1..15}; do
  if curl --fail --silent --show-error \
    --cacert "$local_stage/ca.crt" \
    "https://${server_address}:8443/api/v1/health/ready" \
    >/dev/null 2>&1 \
    && systemctl is-active --quiet easymanage-server.service easymanage-agent.service; then
    server_ready="true"
    break
  fi
  sleep 1
done
if [[ "$server_ready" != "true" ]]; then
  printf 'Local EasyManage server did not become ready within 15 seconds.\n' >&2
  printf 'Inspect it with: journalctl -u easymanage-server.service -n 100 --no-pager\n' >&2
  exit 1
fi
remote_stage="$(
  run_as_operator ssh "${ssh_config_options[@]}" -o BatchMode=yes "$remote_target" \
    'mktemp -d /tmp/easymanage-agent.XXXXXX'
)"
if [[ ! "$remote_stage" =~ ^/tmp/easymanage-agent\.[A-Za-z0-9]+$ ]]; then
  printf 'Remote host returned an unsafe staging path; refusing deployment.\n' >&2
  exit 1
fi

run_as_operator scp "${ssh_config_options[@]}" -pq \
  "$local_stage/easymanage-agent" \
  "$local_stage/ca.crt" \
  "$local_stage/agent.env" \
  "$local_stage/install-agent.sh" \
  "$local_stage/INSTALL_AS_ADMIN.sh" \
  "$local_stage/easymanage-agent.service" \
  "$local_stage/SHA256SUMS" \
  "$remote_target:$remote_stage/"

run_as_operator ssh "${ssh_config_options[@]}" -o BatchMode=yes "$remote_target" \
  "chmod 0700 '$remote_stage' && \
   chmod 0600 '$remote_stage/agent.env' '$remote_stage/ca.crt' '$remote_stage/SHA256SUMS' && \
   chmod 0755 '$remote_stage/easymanage-agent' '$remote_stage/install-agent.sh' \
     '$remote_stage/INSTALL_AS_ADMIN.sh' && \
   chmod 0644 '$remote_stage/easymanage-agent.service'"

if [[ "$remote_install_mode" == "manual" ]]; then
  keep_remote_stage="true"
  printf '\nLocal EasyManage server and agent are active.\n'
  printf 'The server 2 bundle is staged privately at: %s:%s\n' \
    "$remote_target" "$remote_stage"
  printf 'Ask a server 2 administrator to run exactly:\n\n'
  printf '  sudo bash %q\n\n' "$remote_stage/INSTALL_AS_ADMIN.sh"
  printf 'The bundle verifies SHA-256 checksums and removes itself after a successful install.\n'
  exit 0
fi

run_as_operator ssh "${ssh_config_options[@]}" -t "$remote_target" \
  "sudo bash '$remote_stage/INSTALL_AS_ADMIN.sh'"
run_as_operator ssh "${ssh_config_options[@]}" -o BatchMode=yes "$remote_target" \
  'systemctl is-active --quiet easymanage-agent.service'

printf '\nAll EasyManage services are active on the platform host and server 2.\n'
