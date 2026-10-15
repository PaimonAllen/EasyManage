#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# An administrator performs this one-time legacy-root to SSH-user handover.
set -Eeuo pipefail
umask 0077
operator_user="${1:?Existing ordinary SSH user}"
[[ $EUID == 0 && "$operator_user" =~ ^[a-z_][a-z0-9_-]*$ && "$operator_user" != root ]] || exit 64
bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
operator_uid="$(id -u "$operator_user")"
operator_group="$(id -gn "$operator_user")"
agent_base="$operator_home/.easymanage"
[[ "$bundle_dir" =~ ^/tmp/easymanage-agent\.[a-zA-Z0-9_-]+$ || "$bundle_dir" =~ ^"$agent_base"/staging/agent\.[a-zA-Z0-9]{8}$ ]] || exit 64
[[ "$operator_home" =~ ^/home/[a-zA-Z0-9_.-]+$ && "$operator_uid" != 0 ]] || exit 64
assert_plain_path() {
  local item="$1"
  while [[ "$item" != / && "$item" != . ]]; do
    [[ ! -L "$item" ]] || { printf 'Refusing symbolic-link deployment path.\n' >&2; return 1; }
    item="$(dirname -- "$item")"
  done
}
for item in "$bundle_dir" "$agent_base" /etc/easymanage/agent.env /etc/easymanage/tls/ca.crt; do
  assert_plain_path "$item"
done
[[ -f /etc/easymanage/agent.env && -f /etc/easymanage/tls/ca.crt ]] || exit 64
[[ "$(stat -c %u /etc/easymanage/agent.env)" == 0 ]] || exit 64
declare -A verified_files=()
verify_bundle() {
  local directory="$1" line file
  [[ -f "$directory/SHA256SUMS" && ! -L "$directory/SHA256SUMS" ]] || return 1
  verified_files=()
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ "$line" =~ ^[a-fA-F0-9]{64}\ \ ([a-zA-Z0-9_.-]+)$ ]] || return 1
    file="${BASH_REMATCH[1]}"
    case "$file" in
      easymanage-agent|install-user-agent.sh|install-full-candidate.sh|migrate-agent-to-user.sh|handover.sh|easymanage-agent.user.service|node.json|agent.env|ca.crt) ;;
      *) return 1 ;;
    esac
    [[ -z "${verified_files[$file]:-}" && -f "$directory/$file" && ! -L "$directory/$file" ]] || return 1
    verified_files["$file"]=1
  done < "$directory/SHA256SUMS"
  for file in easymanage-agent install-user-agent.sh migrate-agent-to-user.sh handover.sh easymanage-agent.user.service node.json; do
    [[ "${verified_files[$file]:-}" == 1 ]] || return 1
  done
  (cd "$directory" && sha256sum --check --status SHA256SUMS)
}
verify_bundle "$bundle_dir" || { printf 'Staged bundle manifest validation failed.\n' >&2; exit 1; }
# Freeze and re-verify the exact source files before executing any bundled script as root.
root_stage="$(mktemp -d /var/lib/easymanage-agent-handover.XXXXXX)"
for file in "${!verified_files[@]}" SHA256SUMS; do
  runuser -u "$operator_user" -- dd status=none if="$bundle_dir/$file" > "$root_stage/$file"
done
verify_bundle "$root_stage" || { printf 'Staged files changed during handover; no service was changed.\n' >&2; exit 1; }
node_id="$(python3 - "$root_stage/node.json" "$operator_user" "$agent_base" <<'PY'
import json, sys, uuid
with open(sys.argv[1], encoding="utf-8") as source:
    node = json.load(source)
if node.get("user") != sys.argv[2] or node.get("agent_home") != sys.argv[3]:
    raise SystemExit("Bundle account/home differs from the requested SSH user")
print(uuid.UUID(node["node_id"]))
PY
)"
# Treat the legacy environment as data. Never source it or print its secret values.
server_url="$(sed -n 's/^EASYMANAGE_SERVER_URL=//p' /etc/easymanage/agent.env)"
enrollment_token="$(sed -n 's/^EASYMANAGE_ENROLLMENT_TOKEN=//p' /etc/easymanage/agent.env)"
[[ "$server_url" =~ ^https://([a-zA-Z0-9.-]+|\[[a-fA-F0-9:]+\])(:[0-9]+)?$ && "$enrollment_token" =~ ^[a-fA-F0-9]{64}$ ]] || exit 64
install -m 0600 /etc/easymanage/tls/ca.crt "$root_stage/ca.crt"
printf '%s\n' "EASYMANAGE_SERVER_URL=$server_url" \
  "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" "EASYMANAGE_NODE_ID=$node_id" \
  "EASYMANAGE_CA_CERTIFICATE=$agent_base/config/ca.crt" \
  "EASYMANAGE_AGENT_STATE_DB=$agent_base/state/state.db" \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' > "$root_stage/agent.env"
unset enrollment_token
# The manifest must match the populated configuration, including formerly absent env/CA files.
(cd "$root_stage" && sha256sum easymanage-agent install-user-agent.sh install-full-candidate.sh migrate-agent-to-user.sh \
  handover.sh easymanage-agent.user.service node.json agent.env ca.crt > SHA256SUMS)
verify_bundle "$root_stage"
install -d -o "$operator_user" -g "$operator_group" -m 0700 "$agent_base/staging"
user_bundle="$(mktemp -d "$agent_base/staging/agent.XXXXXXXX")"
# Keep the parent root-owned/private while populating it, then relinquish the whole bundle.
for file in "${!verified_files[@]}" SHA256SUMS; do
  install -o "$operator_user" -g "$operator_group" -m 0600 "$root_stage/$file" "$user_bundle/$file"
done
chown "$operator_user:$operator_group" "$user_bundle"
run_user_service() {
  runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" systemctl --user "$@"
}
was_root_active=false
was_root_enabled=false
was_user_active=false
if systemctl is-active --quiet easymanage-agent.service; then was_root_active=true; fi
if systemctl is-enabled --quiet easymanage-agent.service; then was_root_enabled=true; fi
if run_user_service is-active --quiet easymanage-agent.service; then was_user_active=true; fi
handover_started=false
restore_on_exit() {
  local result=$?
  trap - EXIT
  if (( result != 0 )) && [[ "$handover_started" == true ]]; then
    set +e
    run_user_service stop easymanage-agent.service
    if [[ "$was_root_active" == true ]]; then
      run_user_service disable easymanage-agent.service
      if [[ "$was_root_enabled" == true ]]; then systemctl enable easymanage-agent.service; fi
      systemctl start easymanage-agent.service
    elif [[ "$was_user_active" == true ]]; then
      run_user_service start easymanage-agent.service
    fi
    printf 'Handover failed; prior service restoration attempted. Root recovery: %s\n' "$root_stage" >&2
  fi
  exit "$result"
}
trap restore_on_exit EXIT
handover_started=true
bash "$root_stage/migrate-agent-to-user.sh" "$operator_user" "$node_id"
runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
  DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" \
  bash "$user_bundle/install-user-agent.sh"
if systemctl is-active --quiet easymanage-agent.service; then
  printf 'Legacy system Agent unexpectedly restarted; rolling back to avoid duplicate agents.\n' >&2
  exit 1
fi
trap - EXIT
printf 'Agent now runs as %s; complete identity and legacy service retained for recovery.\n' "$operator_user"
printf 'Verified installation bundle: %s\nRoot-owned recovery files: %s\n' "$user_bundle" "$root_stage"
