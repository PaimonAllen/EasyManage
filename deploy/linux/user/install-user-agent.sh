#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# Verified, reversible install as the target SSH user. Never invokes sudo.
set -Eeuo pipefail
umask 0077
repair=false
if (( $# )); then [[ $# == 1 && "$1" == --repair ]] || exit 64; repair=true; fi
if (( EUID == 0 )); then
  printf 'Run this installer as the intended ordinary agent user.\n' >&2
  exit 1
fi
operator_user="$(id -un)"
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
[[ "$operator_home" =~ ^/home/[a-zA-Z0-9_.-]+$ && "${HOME:?}" == "$operator_home" ]] || exit 64
bundle_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
[[ "$bundle_dir" =~ ^/tmp/easymanage-agent\.[a-zA-Z0-9_-]+$ || "$bundle_dir" =~ ^"$operator_home"/\.easymanage/staging/agent\.[a-zA-Z0-9]{8}$ ]] || exit 64
agent_base="$operator_home/.easymanage"
unit_path="$operator_home/.config/systemd/user/easymanage-agent.service"
assert_plain_path() {
  local item="$1"
  while [[ "$item" != / && "$item" != . ]]; do
    [[ ! -L "$item" ]] || { printf 'Refusing symbolic-link deployment path.\n' >&2; return 1; }
    item="$(dirname -- "$item")"
  done
}
for item in "$bundle_dir" "$agent_base/bin/easymanage-agent" "$agent_base/config/agent.env" \
  "$agent_base/config/ca.crt" "$agent_base/state/state.db" "$agent_base/state/state.db-wal" \
  "$agent_base/state/state.db-shm" "$agent_base/bin/agent-user-control.py" "$agent_base/config/agent-launch.json" "$unit_path"; do
  assert_plain_path "$item"
done
cd "$bundle_dir"
[[ -f SHA256SUMS && ! -L SHA256SUMS ]] || exit 64
declare -A verified_files=()
while IFS= read -r line || [[ -n "$line" ]]; do
  [[ "$line" =~ ^[a-fA-F0-9]{64}\ \ ([a-zA-Z0-9_.-]+)$ ]] || exit 64
  file="${BASH_REMATCH[1]}"
  case "$file" in
    easymanage-agent|agent-user-control.py|install-user-agent.sh|install-full-candidate.sh|migrate-agent-to-user.sh|handover.sh|easymanage-agent.user.service|node.json|agent.env|ca.crt) ;;
    *) exit 64 ;;
  esac
  [[ -z "${verified_files[$file]:-}" && -f "$file" && ! -L "$file" ]] || exit 64
  verified_files["$file"]=1
done < SHA256SUMS
for file in easymanage-agent agent-user-control.py install-user-agent.sh easymanage-agent.user.service node.json agent.env ca.crt; do
  [[ "${verified_files[$file]:-}" == 1 ]] || { printf 'Incomplete agent installation manifest.\n' >&2; exit 64; }
done
sha256sum --check --status SHA256SUMS
legacy_state="$(systemctl show -p ActiveState --value easymanage-agent.service 2>/dev/null || true)"
if [[ "$legacy_state" != inactive && "$legacy_state" != failed && -n "$legacy_state" ]] \
  || systemctl is-enabled --quiet easymanage-agent.service 2>/dev/null; then
  printf 'A system/root Agent is active or enabled. An administrator must migrate it first:\n' >&2
  printf '  sudo bash %q %q\n' "$bundle_dir/handover.sh" "$operator_user" >&2
  exit 3
fi
# Validate configuration as plain data, including the persistent identity. Never source/eval it.
python3 - "$agent_base" "$operator_user" <<'PY'
import json, pathlib, re, sqlite3, sys, uuid
from urllib.parse import quote
base, user = sys.argv[1:]
values = {}
for line in pathlib.Path("agent.env").read_text().splitlines():
    if not line or line.startswith("#"):
        continue
    key, separator, value = line.partition("=")
    if not separator or key in values:
        raise SystemExit("Invalid or duplicate agent environment entry")
    values[key] = value
if not re.fullmatch(r"https://(?:[a-zA-Z0-9.-]+|\[[a-fA-F0-9:]+\])(?::[0-9]+)?", values.get("EASYMANAGE_SERVER_URL", "")):
    raise SystemExit("Invalid HTTPS controller URL")
if not re.fullmatch(r"[a-fA-F0-9]{64}", values.get("EASYMANAGE_ENROLLMENT_TOKEN", "")):
    raise SystemExit("Invalid enrollment configuration")
if values.get("EASYMANAGE_CA_CERTIFICATE") != base + "/config/ca.crt" or values.get("EASYMANAGE_AGENT_STATE_DB") != base + "/state/state.db":
    raise SystemExit("Agent configuration points outside the expected user deployment")
node_id = uuid.UUID(values["EASYMANAGE_NODE_ID"])
node = json.loads(pathlib.Path("node.json").read_text())
if uuid.UUID(node["node_id"]) != node_id or node.get("user") != user or node.get("agent_home") != base:
    raise SystemExit("Bundle node/account identity mismatch")
state = pathlib.Path(base) / "state/state.db"
if state.exists():
    with sqlite3.connect("file:" + quote(str(state), safe="/") + "?mode=ro", uri=True, timeout=5) as db:
        existing = db.execute("SELECT value FROM metadata WHERE key='node_id'").fetchone()
    if not existing or uuid.UUID(existing[0]) != node_id:
        raise SystemExit("Persisted identity differs; administrator review is required")
old_env = pathlib.Path(base) / 'config/agent.env'
if old_env.exists():
    import shlex
    old = {}
    for line in old_env.read_text().splitlines():
        parts = shlex.split(line, comments=True)
        if parts and parts[0] == 'export': parts = parts[1:]
        if not parts: continue
        if len(parts) != 1 or '=' not in parts[0]: raise SystemExit('Invalid existing configuration')
        key, value = parts[0].split('=', 1)
        if key in old: raise SystemExit('Duplicate existing configuration')
        old[key] = value
    if old.get('EASYMANAGE_NODE_ID') and uuid.UUID(old['EASYMANAGE_NODE_ID']) != node_id:
        raise SystemExit('Existing configuration belongs to another node')
    if old.get('EASYMANAGE_AGENT_STATE_DB', str(state)) != str(state):
        raise SystemExit('Custom state location requires explicit migration')
PY
backend=compatibility
if [[ "$(loginctl show-user "$(id -u)" -p Linger --value 2>/dev/null || true)" == yes ]] && systemctl --user show-environment >/dev/null 2>&1; then
  backend=systemd
fi
install -d -m 0700 "$agent_base" "$agent_base/bin" "$agent_base/config" "$agent_base/state"
if [[ "$backend" == systemd ]]; then install -d -m 0700 "$operator_home/.config/systemd/user"; fi
backup_dir="$(mktemp -d "$agent_base/install-backup.XXXXXX")"
targets=("$agent_base/bin/easymanage-agent" "$agent_base/config/agent.env" "$agent_base/config/ca.crt" "$unit_path" "$agent_base/bin/agent-user-control.py" "$agent_base/config/agent-launch.json")
sources=("$bundle_dir/easymanage-agent" "$bundle_dir/agent.env" "$bundle_dir/ca.crt" "$bundle_dir/easymanage-agent.user.service" "$bundle_dir/agent-user-control.py")
for index in "${!targets[@]}"; do
  target="${targets[$index]}"
  [[ ! -e "$target" || -f "$target" ]] || exit 64
  if [[ -f "$target" ]]; then cp -p -- "$target" "$backup_dir/previous-$index"; fi
done
was_active=false
was_enabled=false
was_compat=false
was_compat_enabled=false
if [[ -f "$agent_base/state/compat.enabled" ]]; then was_compat_enabled=true; fi
if [[ -f "$agent_base/bin/agent-user-control.py" ]] && python3 "$agent_base/bin/agent-user-control.py" status >/dev/null 2>&1; then was_compat=true; fi
if systemctl --user is-active --quiet easymanage-agent.service; then was_active=true; fi
if systemctl --user is-enabled --quiet easymanage-agent.service; then was_enabled=true; fi
mutation_started=false
restore_on_exit() {
  local result=$?
  trap - EXIT
  if (( result != 0 )) && [[ "$mutation_started" == true ]]; then
    set +e
    if [[ -f "$agent_base/bin/agent-user-control.py" ]]; then python3 "$agent_base/bin/agent-user-control.py" stop; fi
    systemctl --user stop easymanage-agent.service
    for index in "${!targets[@]}"; do
      target="${targets[$index]}"
      if [[ -f "$target" && ! -L "$target" ]]; then mv -- "$target" "$backup_dir/failed-$index"; fi
      if [[ -f "$backup_dir/previous-$index" ]]; then cp -p -- "$backup_dir/previous-$index" "$target"; fi
    done
    # If enrollment never completed, restore only the previous credential;
    # never overwrite a newly issued token or roll back queued/history data.
    python3 - "$agent_base/state/state.db" "$backup_dir/state.db" <<'PY'
import pathlib, sqlite3, sys
from urllib.parse import quote
state, backup = map(pathlib.Path, sys.argv[1:])
if state.is_file() and backup.is_file():
    with sqlite3.connect('file:' + quote(str(backup), safe='/') + '?mode=ro', uri=True) as previous:
        row = previous.execute("SELECT value FROM metadata WHERE key='agent_token'").fetchone()
    if row:
        with sqlite3.connect(state) as current:
            current.execute("INSERT OR IGNORE INTO metadata(key,value) VALUES('agent_token',?)", row)
PY
    systemctl --user daemon-reload
    if [[ "$was_enabled" == true ]]; then
      systemctl --user enable easymanage-agent.service
    else
      systemctl --user disable easymanage-agent.service
    fi
    if [[ "$was_active" == true ]]; then systemctl --user start easymanage-agent.service; fi
    if [[ "$was_compat" == true || "$was_compat_enabled" == true ]]; then python3 "$agent_base/bin/agent-user-control.py" start; fi
    printf 'User-agent update failed; prior files/service restoration attempted. Recovery: %s\n' "$backup_dir" >&2
  fi
  exit "$result"
}
trap restore_on_exit EXIT
mutation_started=true
# Stop only this agent while replacing its executable, configuration and unit together.
systemctl --user stop easymanage-agent.service 2>/dev/null || [[ "$was_active" == false ]]
if [[ -f "$agent_base/bin/agent-user-control.py" ]]; then python3 "$agent_base/bin/agent-user-control.py" stop; fi
# A consistent recovery copy includes committed WAL data. The live database,
# queued reports and collection settings are never replaced/reset. Explicit
# repair renews only the credential, so an invalid retained token is recoverable.
python3 - "$agent_base/state/state.db" "$backup_dir/state.db" "$repair" <<'PY'
import os, pathlib, sqlite3, sys
from urllib.parse import quote
source, backup = map(pathlib.Path, sys.argv[1:3])
if source.exists():
    with sqlite3.connect('file:' + quote(str(source), safe='/') + '?mode=ro', uri=True, timeout=5) as original:
        with sqlite3.connect(backup) as recovery:
            original.backup(recovery)
    os.chmod(backup, 0o600)
    if sys.argv[3] == 'true':
        with sqlite3.connect(source) as current:
            current.execute("DELETE FROM metadata WHERE key='agent_token'")
PY
for index in "${!sources[@]}"; do
  if [[ "$backend" != systemd && "$index" == 3 ]]; then continue; fi
  target="${targets[$index]}"
  temporary="$(mktemp "$(dirname -- "$target")/.easymanage-install.XXXXXX")"
  mode=0600
  if [[ "$index" == 0 || "$index" == 4 ]]; then mode=0700; fi
  install -m "$mode" "${sources[$index]}" "$temporary"
  mv -f -- "$temporary" "$target"
done
if [[ "$backend" == systemd ]]; then
  python3 - "$agent_base/config/agent-launch.json" <<'PY'
import json,pathlib,sys
pathlib.Path(sys.argv[1]).write_text(json.dumps({'backend':'systemd','autostart':'systemd'}))
PY
  chmod 600 "$agent_base/config/agent-launch.json"
  systemctl --user daemon-reload
  systemctl --user enable easymanage-agent.service
  systemctl --user restart easymanage-agent.service
  sleep 3
  systemctl --user is-active --quiet easymanage-agent.service
else
  if [[ -f "$unit_path" ]]; then
    systemctl --user disable easymanage-agent.service >/dev/null 2>&1 || true
    # Retire the previous user unit even when its manager cannot be reached.
    # Leaving an enabled unit on disk could launch a duplicate at next login.
    mv -- "$unit_path" "$backup_dir/retired-user-unit"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
  fi
  python3 "$agent_base/bin/agent-user-control.py" configure
  python3 "$agent_base/bin/agent-user-control.py" start
  sleep 3
  python3 "$agent_base/bin/agent-user-control.py" status
fi
trap - EXIT
printf 'Basic Agent installed for user %s using %s. Master checks the fresh heartbeat.\n' "$operator_user" "$backend"
