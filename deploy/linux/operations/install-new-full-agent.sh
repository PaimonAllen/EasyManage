#!/usr/bin/env bash
# Embedded controller-owned first install or explicit same-UUID Full repair.
# Invoked through root SSH or sudo with a clean environment.
set -Eeuo pipefail
exec </dev/null
umask 0077
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
[[ $EUID == 0 && ( $# == 6 || $# == 7 ) ]] || exit 64
source_stage="$1"
binary_hash="$2"
ca_hash="$3"
env_hash="$4"
node_id="$5"
server_url="$6"
install_mode="${7:-new}"
[[ "$install_mode" == new || "$install_mode" == repair ]] || exit 64
[[ "$source_stage" =~ ^(/home/[a-zA-Z0-9_.-]+|/root)/\.easymanage/staging/agent\.[a-zA-Z0-9]{8}$ ]] || exit 64
for hash in "$binary_hash" "$ca_hash" "$env_hash"; do [[ "$hash" =~ ^[a-f0-9]{64}$ ]] || exit 64; done
[[ "$node_id" =~ ^[a-f0-9-]{36}$ && "$server_url" =~ ^https://([a-zA-Z0-9.-]+|\[[a-fA-F0-9:]+\]):[0-9]+/?$ ]] || exit 64
plain_path() {
  local cursor="$1"
  while [[ "$cursor" != / ]]; do [[ ! -L "$cursor" ]] || return 1; cursor="$(dirname -- "$cursor")"; done
}
root_path() {
  local cursor="$1" mode
  plain_path "$cursor" || return 1
  while [[ "$cursor" != / ]]; do
    if [[ -e "$cursor" ]]; then
      [[ -d "$cursor" && "$(stat -c %u -- "$cursor")" == 0 ]] || return 1
      mode="$(stat -c %a -- "$cursor")"
      (( (8#$mode & 0022) == 0 )) || return 1
    fi
    cursor="$(dirname -- "$cursor")"
  done
}
for directory in /var/lib /etc/easymanage/tls /usr/local/sbin /etc/systemd/system /run/easymanage-first-install; do root_path "$directory"; done
plain_path "$source_stage"
for item in easymanage-agent agent.env ca.crt; do
  [[ -f "$source_stage/$item" && ! -L "$source_stage/$item" ]]
done
[[ "$(stat -c %s "$source_stage/easymanage-agent")" -le 268435456
   && "$(stat -c %s "$source_stage/agent.env")" -le 65536
   && "$(stat -c %s "$source_stage/ca.crt")" -le 1048576 ]]
install -d -m 0700 /run/easymanage-first-install
[[ ! -L /run/easymanage-first-install/lock ]]
exec 9>/run/easymanage-first-install/lock
flock -n 9
# First installation remains strict. Only original-node redeployment may repair.
loaded="$(systemctl show easymanage-agent.service -p LoadState --value)"
if [[ "$install_mode" == new ]]; then
  [[ "$loaded" == not-found ]]
  for target in /var/lib/easymanage-agent /usr/local/sbin/easymanage-agent /etc/easymanage/agent.env /etc/systemd/system/easymanage-agent.service /etc/easymanage/tls/ca.crt; do
    [[ ! -e "$target" && ! -L "$target" ]]
  done
else
  root_path /var/lib/easymanage-agent
  for target in /usr/local/sbin/easymanage-agent /etc/easymanage/agent.env /etc/systemd/system/easymanage-agent.service /etc/easymanage/tls/ca.crt /var/lib/easymanage-agent/state.db /var/lib/easymanage-agent/state.db-wal /var/lib/easymanage-agent/state.db-shm; do
    plain_path "$target"
    if [[ -e "$target" ]]; then
      [[ -f "$target" && "$(stat -c %u "$target")" == 0 && "$(stat -c %h "$target")" == 1 ]]
      mode="$(stat -c %a "$target")"; (( (8#$mode & 0022) == 0 ))
    fi
  done
  if [[ "$loaded" == loaded ]]; then
    service_user="$(systemctl show easymanage-agent.service -p User --value)"
    [[ -z "$service_user" || "$service_user" == root ]]
  else [[ "$loaded" == not-found ]]; fi
fi
recovery="$(mktemp -d /var/lib/easymanage-first-install.XXXXXXXX)"
for item in easymanage-agent agent.env ca.crt; do install -m 0600 "$source_stage/$item" "$recovery/$item"; done
printf '%s  %s\n' "$binary_hash" "$recovery/easymanage-agent" "$ca_hash" "$recovery/ca.crt" "$env_hash" "$recovery/agent.env" | sha256sum --check --status
# Parse data, never source/eval a file writable by the SSH account. Rewrite all
# Full persistent state/config paths to the root-owned system locations.
python3 - "$recovery" "$node_id" "$server_url" "$install_mode" <<'PY'
import pathlib, re, shlex, sqlite3, sys, uuid
root, node, url = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
uuid.UUID(node)
if sys.argv[4] == 'repair':
    state = pathlib.Path('/var/lib/easymanage-agent/state.db')
    if state.exists():
        with sqlite3.connect('file:' + str(state) + '?mode=ro', uri=True, timeout=3) as db:
            identity = db.execute("SELECT value FROM metadata WHERE key='node_id'").fetchone()
        if not identity or uuid.UUID(identity[0]) != uuid.UUID(node):
            raise SystemExit('Retained Full identity belongs to another node')
    old_env = pathlib.Path('/etc/easymanage/agent.env')
    old = {}
    if old_env.exists():
        for line in old_env.read_text().splitlines():
            parts = shlex.split(line, comments=True)
            if parts and parts[0] == 'export': parts = parts[1:]
            if not parts: continue
            if len(parts) != 1 or '=' not in parts[0]: raise SystemExit('Invalid retained Full configuration')
            key, value = parts[0].split('=', 1)
            if key in old: raise SystemExit('Duplicate retained Full configuration')
            old[key] = value
        if old.get('EASYMANAGE_NODE_ID') and uuid.UUID(old['EASYMANAGE_NODE_ID']) != uuid.UUID(node):
            raise SystemExit('Retained Full configuration belongs to another node')
        if not old.get('EASYMANAGE_NODE_ID') and not state.exists():
            raise SystemExit('Retained Full identity cannot be verified')
        if old.get('EASYMANAGE_AGENT_STATE_DB', str(state)) != str(state):
            raise SystemExit('Custom Full state location requires explicit migration')
values = {}
for line in (root / 'agent.env').read_text().splitlines():
    key, separator, value = line.partition('=')
    if not separator or key in values:
        raise SystemExit('Invalid deployment configuration')
    values[key] = value
if values.get('EASYMANAGE_NODE_ID') != node or values.get('EASYMANAGE_SERVER_URL') != url:
    raise SystemExit('Deployment identity/address mismatch')
if not re.fullmatch(r'[a-fA-F0-9]{64}', values.get('EASYMANAGE_ENROLLMENT_TOKEN', '')):
    raise SystemExit('Invalid scoped enrollment token')
values = {'EASYMANAGE_SERVER_URL':url, 'EASYMANAGE_NODE_ID':node,
          'EASYMANAGE_ENROLLMENT_TOKEN':values['EASYMANAGE_ENROLLMENT_TOKEN'],
          'EASYMANAGE_CA_CERTIFICATE':'/etc/easymanage/tls/ca.crt',
          'EASYMANAGE_AGENT_STATE_DB':'/var/lib/easymanage-agent/state.db',
          'EASYMANAGE_REPORT_INTERVAL_SECONDS':'10'}
(root / 'full.env').write_text(''.join(key + '=' + value + '\n' for key, value in values.items()))
PY
cat > "$recovery/full.service" <<'UNIT'
[Unit]
Description=EasyManage Full monitoring agent
After=network-online.target
Wants=network-online.target
[Service]
Type=simple
User=root
Group=root
WorkingDirectory=/var/lib/easymanage-agent
Environment=EASYMANAGE_ENV_FILE=/etc/easymanage/agent.env
Environment=EASYMANAGE_ADMIN_POLICY=require
ExecStart=/usr/local/sbin/easymanage-agent
Restart=on-failure
RestartSec=10
TimeoutStopSec=5
UMask=0077
Nice=10
TasksMax=64
MemoryMax=256M
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/var/lib/easymanage-agent
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
[Install]
WantedBy=multi-user.target
UNIT
mutated=false
completed=false
was_active=false
was_enabled=false
targets=(/usr/local/sbin/easymanage-agent /etc/easymanage/agent.env /etc/easymanage/tls/ca.crt /etc/systemd/system/easymanage-agent.service)
if [[ "$install_mode" == repair ]]; then
  for index in "${!targets[@]}"; do
    [[ ! -f "${targets[$index]}" ]] || cp -p -- "${targets[$index]}" "$recovery/previous-$index"
  done
  if systemctl is-active --quiet easymanage-agent.service; then was_active=true; fi
  if systemctl is-enabled --quiet easymanage-agent.service; then was_enabled=true; fi
fi
recover() {
  local status=$? target
  trap - EXIT
  if [[ "$completed" != true && "$mutated" == true ]]; then
    set +e
    timeout 8s systemctl stop easymanage-agent.service
    timeout 8s systemctl disable easymanage-agent.service
    install -d -m 0700 "$recovery/failed"
    for target in /usr/local/sbin/easymanage-agent /etc/easymanage/agent.env /etc/easymanage/tls/ca.crt /etc/systemd/system/easymanage-agent.service; do
      if [[ -f "$target" && ! -L "$target" ]]; then mv -- "$target" "$recovery/failed/$(basename -- "$target")"; fi
    done
    if [[ "$install_mode" == repair ]]; then
      for index in "${!targets[@]}"; do
        [[ ! -f "$recovery/previous-$index" ]] || cp -p -- "$recovery/previous-$index" "${targets[$index]}"
      done
      python3 - "$recovery/state.db" <<'PY'
import pathlib, sqlite3, sys
backup = pathlib.Path(sys.argv[1])
state = pathlib.Path('/var/lib/easymanage-agent/state.db')
if backup.is_file() and state.is_file():
    with sqlite3.connect('file:' + str(backup) + '?mode=ro', uri=True) as previous:
        row = previous.execute("SELECT value FROM metadata WHERE key='agent_token'").fetchone()
    if row:
        with sqlite3.connect(state) as current:
            current.execute("INSERT OR IGNORE INTO metadata(key,value) VALUES('agent_token',?)", row)
PY
    elif [[ -d /var/lib/easymanage-agent && ! -L /var/lib/easymanage-agent ]]; then mv -- /var/lib/easymanage-agent "$recovery/failed/state"; fi
    timeout 8s systemctl daemon-reload
    if [[ "$was_enabled" == true ]]; then timeout 8s systemctl enable easymanage-agent.service; fi
    if [[ "$was_active" == true ]]; then timeout 8s systemctl start easymanage-agent.service; fi
    printf 'Full installation failed; new files/state retained for recovery: %s\n' "$recovery" >&2
    [[ $status != 0 ]] || status=1
  fi
  exit "$status"
}
trap recover EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
mutated=true
if [[ "$install_mode" == repair && "$loaded" == loaded ]]; then timeout 15s systemctl stop easymanage-agent.service; fi
python3 - "$recovery" "$install_mode" <<'PY'
import os, pathlib, sqlite3, sys
source = pathlib.Path('/var/lib/easymanage-agent/state.db')
if source.exists():
    destination = pathlib.Path(sys.argv[1]) / 'state.db'
    with sqlite3.connect('file:' + str(source) + '?mode=ro', uri=True, timeout=5) as original:
        with sqlite3.connect(destination) as recovery:
            original.backup(recovery)
    os.chmod(destination, 0o600)
    if sys.argv[2] == 'repair':
        with sqlite3.connect(source) as current:
            current.execute("DELETE FROM metadata WHERE key='agent_token'")
PY
install -d -m 0700 /var/lib/easymanage-agent /etc/easymanage /etc/easymanage/tls
install -m 0755 "$recovery/easymanage-agent" /usr/local/sbin/easymanage-agent
install -m 0600 "$recovery/ca.crt" /etc/easymanage/tls/ca.crt
install -m 0600 "$recovery/full.env" /etc/easymanage/agent.env
install -m 0644 "$recovery/full.service" /etc/systemd/system/easymanage-agent.service
timeout 8s systemctl daemon-reload
timeout 8s systemctl enable easymanage-agent.service
timeout 15s systemctl start easymanage-agent.service
sleep 3
timeout 8s systemctl is-active --quiet easymanage-agent.service
completed=true
printf 'Full system service started; SQLite UUID/history retained. Explicit repair renews enrollment only. Await fresh UID 0 heartbeat. Recovery: %s\n' "$recovery"
