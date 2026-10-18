#!/usr/bin/env bash
# Embedded in the controller and invoked with a clean environment, never sourced
# from a file writable by the basic Agent account. Recovery files are retained.
set -Eeuo pipefail
exec </dev/null
umask 0077
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
[[ $# == 6 && $EUID == 0 ]] || exit 64
agent_base="$1"
operator_user="$2"
expected_hash="${3,,}"
expected_node="$4"
expected_url="$5"
expected_ca="${6,,}"
[[ "$operator_user" =~ ^[a-z_][a-z0-9_-]*$ && "$operator_user" != root ]] || exit 64
[[ "$expected_hash" =~ ^[a-f0-9]{64}$ && "$expected_ca" =~ ^[a-f0-9]{64}$ ]] || exit 64
[[ "$expected_node" =~ ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$ ]] || exit 64
[[ "$expected_url" =~ ^https://([a-zA-Z0-9.-]+|\[[a-fA-F0-9:]+\]):[0-9]+/?$ ]] || exit 64
started_seconds=$SECONDS
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
operator_uid="$(id -u "$operator_user")"
[[ "$operator_home" == "/home/$operator_user" && "$agent_base" == "$operator_home/.easymanage" ]] || exit 64

die() { printf '%s\n' "$1" >&2; exit 1; }
no_symlink_chain() {
  local cursor="$1"
  while [[ "$cursor" != / ]]; do
    [[ ! -L "$cursor" ]] || return 1
    cursor="$(dirname -- "$cursor")"
  done
}
root_directory() {
  local cursor="$1" mode
  no_symlink_chain "$cursor" || return 1
  while [[ "$cursor" != / ]]; do
    if [[ -e "$cursor" ]]; then
      [[ -d "$cursor" && "$(stat -c %u -- "$cursor")" == 0 ]] || return 1
      mode="$(stat -c %a -- "$cursor")"
      (( (8#$mode & 0022) == 0 )) || return 1
    fi
    cursor="$(dirname -- "$cursor")"
  done
}
regular_file() {
  [[ -f "$1" && ! -L "$1" && "$(stat -c %s -- "$1")" -le "$2" ]]
}
read_setting() {
  # Duplicate values retain newlines and fail the caller's exact/regex checks.
  sed -n "s/^$2=//p" "$1"
}
bounded() {
  local budget=$((55 - SECONDS + started_seconds))
  (( budget > 0 )) || return 124
  (( budget <= 8 )) || budget=8
  timeout --signal=TERM --kill-after=1s "${budget}s" "$@"
}
compat_present=false
compat_control() {
  timeout --signal=TERM --kill-after=1s 22s runuser -u "$operator_user" -- env -i HOME="$operator_home" PATH=/usr/sbin:/usr/bin:/sbin:/bin \
    /usr/bin/python3 "$agent_base/bin/agent-user-control.py" "$@"
}
user_service() {
  if [[ "$compat_present" == true && ( "$1" == stop || "$1" == disable ) ]]; then
    local loaded
    loaded="$(bounded runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" systemctl --user show easymanage-agent.service -p LoadState --value 2>/dev/null || true)"
    [[ "$loaded" == loaded ]] || return 0
  fi
  bounded runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" systemctl --user "$@"
}
full_service() { bounded systemctl "$@"; }

for directory in "$agent_base" "$agent_base/bin" "$agent_base/config" "$agent_base/state"; do
  if [[ ! -d "$directory" ]] || ! no_symlink_chain "$directory"; then die 'Unsafe or missing basic Agent directory.'; fi
done
regular_file "$agent_base/bin/easymanage-agent" 268435456 || die 'Invalid staged Agent binary.'
regular_file "$agent_base/config/agent.env" 65536 || die 'Invalid staged Agent configuration.'
regular_file "$agent_base/config/ca.crt" 1048576 || die 'Invalid staged Agent CA.'
if regular_file "$agent_base/config/agent-launch.json" 65536 && \
  /usr/bin/python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d.get("backend")=="compatibility" else 1)' "$agent_base/config/agent-launch.json"; then
  regular_file "$agent_base/bin/agent-user-control.py" 65536 || die 'Missing compatibility control.'
  compat_present=true
fi
for directory in /var/lib /etc/easymanage /etc/easymanage/tls /var/lib/easymanage-agent /usr/local/sbin /etc/systemd/system /run/easymanage-promotion; do
  root_directory "$directory" || die 'A destination directory is not root-owned or has unsafe permissions.'
done
install -d -m 0700 /run/easymanage-promotion
[[ ! -L /run/easymanage-promotion/lock ]] || die 'Unsafe promotion lock.'
exec 9>/run/easymanage-promotion/lock
flock -n 9 || die 'Another Agent promotion is in progress.'
root_stage="$(mktemp -d /var/lib/easymanage-promotion.XXXXXX)"
install -m 0700 "$agent_base/bin/easymanage-agent" "$root_stage/easymanage-agent"
printf '%s  %s\n' "$expected_hash" "$root_stage/easymanage-agent" | sha256sum --check --status || die 'Agent binary differs from the trusted release.'
install -m 0600 "$agent_base/config/agent.env" "$root_stage/source.env"
install -m 0600 "$agent_base/config/ca.crt" "$root_stage/ca.crt"
printf '%s  %s\n' "$expected_ca" "$root_stage/ca.crt" | sha256sum --check --status || die 'Agent CA differs from the trusted deployment.'
server_url="$(read_setting "$root_stage/source.env" EASYMANAGE_SERVER_URL)"
enrollment_token="$(read_setting "$root_stage/source.env" EASYMANAGE_ENROLLMENT_TOKEN)"
node_id="$(read_setting "$root_stage/source.env" EASYMANAGE_NODE_ID)"
[[ "$server_url" == "$expected_url" && "$node_id" == "$expected_node" ]] || die 'Agent identity or controller URL differs from the trusted deployment.'
[[ "$enrollment_token" =~ ^[a-fA-F0-9]{64}$ ]] || die 'Invalid Agent enrollment configuration.'

artifact_paths=(/usr/local/sbin/easymanage-agent /etc/systemd/system/easymanage-agent.service /etc/easymanage/agent.env /etc/easymanage/tls/ca.crt)
for target in "${artifact_paths[@]}"; do
  no_symlink_chain "$target" || die 'Unsafe destination artifact.'
  if [[ -e "$target" ]]; then
    [[ -f "$target" && "$(stat -c %u -- "$target")" == 0 ]] || die 'Destination artifact is not a root-owned regular file.'
    install -D -m 0600 "$target" "$root_stage/previous$target"
    stat -c %a -- "$target" > "$root_stage/previous$target.mode"
  fi
done
for state_name in state.db state.db-wal state.db-shm; do
  for directory in "$agent_base/state" /var/lib/easymanage-agent; do
    target="$directory/$state_name"
    no_symlink_chain "$target" || die 'Unsafe Agent identity path.'
    if [[ -e "$target" ]]; then
      regular_file "$target" 67108864 || die 'Agent state is not a bounded regular file.'
    fi
  done
done
basic_active=false
basic_enabled=false
compat_enabled=false
if [[ "$compat_present" == true && -f "$agent_base/state/compat.enabled" ]]; then compat_enabled=true; fi
full_active=false
full_enabled=false
if user_service is-active --quiet easymanage-agent.service; then basic_active=true; fi
if [[ "$(user_service is-enabled easymanage-agent.service 2>/dev/null || true)" == enabled ]]; then basic_enabled=true; fi
if full_service is-active --quiet easymanage-agent.service; then full_active=true; fi
if [[ "$(full_service is-enabled easymanage-agent.service 2>/dev/null || true)" == enabled ]]; then full_enabled=true; fi
existing_node=''
if [[ -f /etc/easymanage/agent.env ]]; then
  existing_node="$(read_setting /etc/easymanage/agent.env EASYMANAGE_NODE_ID)"
fi
if [[ "$full_active" == true && "$existing_node" != "$expected_node" ]]; then
  die 'An active system Agent has a different identity; administrator reconciliation is required.'
fi

mutation_started=false
artifacts_changed=false
state_changed=false
basic_changed=false
full_changed=false
completed=false
rollback() {
  local rollback_started=$SECONDS recovery_ok=true target state_name mode
  rollback_command() {
    local budget=$((25 - SECONDS + rollback_started))
    (( budget > 0 )) || return 124
    (( budget <= 4 )) || budget=4
    timeout --signal=TERM --kill-after=1s "${budget}s" "$@"
  }
  rollback_user() {
    rollback_command runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" systemctl --user "$@"
  }
  if [[ "$full_changed" == true ]]; then rollback_command systemctl stop easymanage-agent.service || recovery_ok=false; fi
  if [[ "$artifacts_changed" == true ]]; then
    for target in "${artifact_paths[@]}"; do
      if [[ -f "$root_stage/previous$target" ]]; then
        mode="$(< "$root_stage/previous$target.mode")"
        install -m "$mode" "$root_stage/previous$target" "$target" || recovery_ok=false
      elif [[ -f "$target" ]]; then
        install -d -m 0700 "$root_stage/failed$(dirname -- "$target")"
        mv -- "$target" "$root_stage/failed$target" || recovery_ok=false
      fi
    done
  fi
  if [[ "$state_changed" == true ]]; then
    install -d -m 0700 "$root_stage/failed-state"
    for state_name in state.db state.db-wal state.db-shm; do
      target="/var/lib/easymanage-agent/$state_name"
      if [[ -f "$target" ]]; then mv -- "$target" "$root_stage/failed-state/$state_name" || recovery_ok=false; fi
      if [[ -f "$root_stage/previous-state/$state_name" ]]; then
        install -m 0600 "$root_stage/previous-state/$state_name" "$target" || recovery_ok=false
      fi
    done
  fi
  if [[ "$full_changed" == true ]]; then
    rollback_command systemctl daemon-reload || recovery_ok=false
    if [[ "$full_enabled" == true ]]; then
      rollback_command systemctl enable easymanage-agent.service || recovery_ok=false
    else
      rollback_command systemctl disable easymanage-agent.service || recovery_ok=false
    fi
    if [[ "$full_active" == true ]]; then rollback_command systemctl start easymanage-agent.service || recovery_ok=false; fi
  fi
  if [[ "$basic_changed" == true ]]; then
    if [[ "$basic_enabled" == true ]]; then rollback_user enable easymanage-agent.service || recovery_ok=false; fi
    if [[ "$basic_active" == true ]]; then rollback_user start easymanage-agent.service || recovery_ok=false; fi
    if [[ "$compat_enabled" == true ]]; then compat_control start || recovery_ok=false; fi
  fi
  if [[ "$recovery_ok" == true ]]; then
    printf 'Promotion did not complete; prior Agent state restored. Recovery files: %s\n' "$root_stage" >&2
  else
    printf 'Promotion did not complete; manual recovery is required. Recovery files: %s\n' "$root_stage" >&2
  fi
}
finish() {
  local status="$1"
  trap - ERR EXIT HUP INT TERM
  if [[ "$completed" != true && "$mutation_started" == true ]]; then
    set +e
    rollback
    [[ "$status" != 0 ]] || status=1
  fi
  exit "$status"
}
trap 'exit 1' ERR
trap 'finish "$?"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

# Compare both the installed and running binary, CA and identity before accepting
# an existing Full service. A stale process or release is replaced with backup.
already_current=false
if [[ "$full_active" == true && -f /usr/local/sbin/easymanage-agent && -f /etc/easymanage/tls/ca.crt ]]; then
  full_pid="$(full_service show -p MainPID --value easymanage-agent.service)"
  full_user="$(full_service show -p User --value easymanage-agent.service)"
  if [[ "$full_pid" =~ ^[1-9][0-9]*$ && ( "$full_user" == root || -z "$full_user" ) ]] \
      && [[ "$(read_setting /etc/easymanage/agent.env EASYMANAGE_SERVER_URL)" == "$expected_url" ]] \
      && printf '%s  %s\n' "$expected_hash" /usr/local/sbin/easymanage-agent "$expected_hash" "/proc/$full_pid/exe" "$expected_ca" /etc/easymanage/tls/ca.crt | sha256sum --check --status; then
    already_current=true
  fi
fi
mutation_started=true
basic_changed=true
if [[ "$compat_present" == true ]]; then compat_control stop || die 'Compatibility Agent did not stop; identity was not copied.'; fi
user_service stop easymanage-agent.service
if [[ "$already_current" == true ]]; then
  user_service disable easymanage-agent.service
  completed=true
  printf 'Matching Full Agent is running; await a fresh Full heartbeat. Recovery files: %s\n' "$root_stage"
  exit 0
fi
full_changed=true
if [[ "$full_active" == true ]]; then full_service stop easymanage-agent.service; fi
install -d -m 0700 /etc/easymanage /etc/easymanage/tls /var/lib/easymanage-agent "$root_stage/previous-state" "$root_stage/new-state"
for state_name in state.db state.db-wal state.db-shm; do
  target="/var/lib/easymanage-agent/$state_name"
  if [[ -f "$target" ]]; then install -m 0600 "$target" "$root_stage/previous-state/$state_name"; fi
  if [[ "$full_active" == true ]]; then source_state="$target"; else source_state="$agent_base/state/$state_name"; fi
  no_symlink_chain "$source_state" || die 'Agent identity path changed during promotion.'
  if [[ -e "$source_state" ]]; then
    regular_file "$source_state" 67108864 || die 'Agent state changed during promotion.'
    install -m 0600 "$source_state" "$root_stage/new-state/$state_name"
  fi
done
state_changed=true
for state_name in state.db state.db-wal state.db-shm; do
  target="/var/lib/easymanage-agent/$state_name"
  if [[ -f "$target" ]]; then mv -- "$target" "$root_stage/old-$state_name"; fi
  if [[ -f "$root_stage/new-state/$state_name" ]]; then install -m 0600 "$root_stage/new-state/$state_name" "$target"; fi
done
artifacts_changed=true
next_binary="$(mktemp /usr/local/sbin/.easymanage-agent.XXXXXX)"
install -m 0755 "$root_stage/easymanage-agent" "$next_binary"
mv -f -- "$next_binary" /usr/local/sbin/easymanage-agent
install -m 0600 "$root_stage/ca.crt" /etc/easymanage/tls/ca.crt
printf '%s\n' "EASYMANAGE_SERVER_URL=$server_url" "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" \
  "EASYMANAGE_NODE_ID=$node_id" 'EASYMANAGE_CA_CERTIFICATE=/etc/easymanage/tls/ca.crt' \
  'EASYMANAGE_AGENT_STATE_DB=/var/lib/easymanage-agent/state.db' \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' > "$root_stage/agent.env"
install -m 0600 "$root_stage/agent.env" /etc/easymanage/agent.env
unset enrollment_token
printf '%s\n' '[Unit]' 'Description=EasyManage Full monitoring agent' 'After=network-online.target' \
  '[Service]' 'Type=simple' 'User=root' 'Group=root' \
  'Environment=EASYMANAGE_ENV_FILE=/etc/easymanage/agent.env' 'Environment=EASYMANAGE_ADMIN_POLICY=require' \
  'ExecStart=/usr/local/sbin/easymanage-agent' 'WorkingDirectory=/var/lib/easymanage-agent' \
  'Restart=on-failure' 'RestartSec=10' 'TimeoutStartSec=5' 'TimeoutStopSec=5' \
  'UMask=0077' 'Nice=10' 'TasksMax=64' 'MemoryMax=256M' \
  'NoNewPrivileges=true' 'ProtectSystem=strict' 'ProtectHome=read-only' \
  'ReadWritePaths=/var/lib/easymanage-agent' 'ProtectKernelTunables=true' \
  'ProtectKernelModules=true' 'ProtectControlGroups=true' \
  '[Install]' 'WantedBy=multi-user.target' > "$root_stage/easymanage-agent.service"
install -m 0644 "$root_stage/easymanage-agent.service" /etc/systemd/system/easymanage-agent.service
full_service daemon-reload
full_service enable easymanage-agent.service
full_service restart easymanage-agent.service
sleep 3
full_service is-active --quiet easymanage-agent.service
user_service disable easymanage-agent.service
completed=true
printf 'Full Agent service started; await a fresh Full heartbeat. Recovery files: %s\n' "$root_stage"
