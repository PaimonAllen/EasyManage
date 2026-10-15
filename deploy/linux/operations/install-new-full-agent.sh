#!/usr/bin/env bash
# Embedded controller-owned code: first installation only, never upgrades an
# existing identity. Invoked through root SSH or sudo with a clean environment.
set -Eeuo pipefail
exec </dev/null
umask 0077
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
[[ $EUID == 0 && $# == 6 ]] || exit 64
source_stage="$1"
binary_hash="$2"
ca_hash="$3"
env_hash="$4"
node_id="$5"
server_url="$6"
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
# Recheck under root immediately before mutation. No first-install operation is
# permitted to replace a stopped, disabled or otherwise existing Agent.
[[ "$(systemctl show easymanage-agent.service -p LoadState --value)" == not-found ]]
for target in /var/lib/easymanage-agent /usr/local/sbin/easymanage-agent /etc/easymanage/agent.env /etc/systemd/system/easymanage-agent.service; do
  [[ ! -e "$target" && ! -L "$target" ]]
done
[[ ! -e /etc/easymanage/tls/ca.crt && ! -L /etc/easymanage/tls/ca.crt ]]
recovery="$(mktemp -d /var/lib/easymanage-first-install.XXXXXXXX)"
for item in easymanage-agent agent.env ca.crt; do install -m 0600 "$source_stage/$item" "$recovery/$item"; done
printf '%s  %s\n' "$binary_hash" "$recovery/easymanage-agent" "$ca_hash" "$recovery/ca.crt" "$env_hash" "$recovery/agent.env" | sha256sum --check --status
# Parse data, never source/eval a file writable by the SSH account. Rewrite all
# Full persistent state/config paths to the root-owned system locations.
python3 - "$recovery" "$node_id" "$server_url" <<'PY'
import pathlib, re, sys, uuid
root, node, url = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
uuid.UUID(node)
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
    if [[ -d /var/lib/easymanage-agent && ! -L /var/lib/easymanage-agent ]]; then mv -- /var/lib/easymanage-agent "$recovery/failed/state"; fi
    timeout 8s systemctl daemon-reload
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
printf 'New Full system service started; controller must confirm UID 0 heartbeat. Recovery: %s\n' "$recovery"
