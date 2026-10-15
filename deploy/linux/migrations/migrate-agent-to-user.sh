#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# One-time, reversible handover of an existing EasyManage root agent.
set -Eeuo pipefail
umask 0077
operator_user="${1:?Specify the existing SSH user}"
expected_node="${2:-}"
[[ $EUID == 0 && "$operator_user" =~ ^[a-z_][a-z0-9_-]*$ && "$operator_user" != root ]] || exit 64
[[ -z "$expected_node" || "$expected_node" =~ ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$ ]] || exit 64
operator_uid="$(id -u "$operator_user")"
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
[[ "$operator_home" =~ ^/home/[a-zA-Z0-9_.-]+$ && "$operator_uid" != 0 ]] || exit 64
agent_base="$operator_home/.easymanage"
assert_plain_path() {
  local item="$1"
  while [[ "$item" != / && "$item" != . ]]; do
    [[ ! -L "$item" ]] || { printf 'Refusing symbolic-link deployment path.\n' >&2; return 1; }
    item="$(dirname -- "$item")"
  done
}
for item in "$agent_base/state/state.db" "$agent_base/state/state.db-wal" "$agent_base/state/state.db-shm" \
  /var/lib/easymanage-agent/state.db /var/lib/easymanage-agent/state.db-wal /var/lib/easymanage-agent/state.db-shm; do
  assert_plain_path "$item"
  [[ ! -e "$item" || -f "$item" ]] || exit 64
done
run_user_service() {
  runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" systemctl --user "$@"
}
run_operator() {
  runuser -u "$operator_user" -- "$@"
}
write_user_state() {
  # Root supplies bytes on stdin; every operation under the user-controlled home uses that UID.
  # shellcheck disable=SC2016
  run_operator bash -c 'set -e; umask 077; temporary=$(mktemp "$1.next.XXXXXX"); trap '\''unlink "$temporary"'\'' EXIT; dd status=none of="$temporary"; mv -T "$temporary" "$1"; trap - EXIT' bash "$1"
}
was_root_active=false
was_user_active=false
was_root_enabled=false
if systemctl is-active --quiet easymanage-agent.service; then was_root_active=true; fi
if run_user_service is-active --quiet easymanage-agent.service; then was_user_active=true; fi
if systemctl is-enabled --quiet easymanage-agent.service; then was_root_enabled=true; fi
backup_dir="$(mktemp -d /var/lib/easymanage-agent-migration.XXXXXX)"
install -d -m 0700 "$backup_dir/previous-user-state" "$backup_dir/failed-user-state"
mutation_started=false
state_replaced=false
previous_state_names=()
restore_on_exit() {
  local result=$?
  trap - EXIT
  if (( result != 0 )) && [[ "$mutation_started" == true ]]; then
    set +e
    run_user_service stop easymanage-agent.service
    if [[ "$state_replaced" == true ]]; then
      for name in state.db state.db-wal state.db-shm; do
        if [[ -e "$agent_base/state/$name" && ! -L "$agent_base/state/$name" ]]; then
          run_operator dd status=none if="$agent_base/state/$name" > "$backup_dir/failed-user-state/$name"
          run_operator unlink "$agent_base/state/$name"
        fi
      done
      for name in "${previous_state_names[@]}"; do
        if [[ -f "$backup_dir/previous-user-state/$name" ]]; then
          write_user_state "$agent_base/state/$name" < "$backup_dir/previous-user-state/$name"
        fi
      done
    fi
    if [[ "$was_root_enabled" == true ]]; then systemctl enable easymanage-agent.service; fi
    if [[ "$was_root_active" == true ]]; then
      systemctl start easymanage-agent.service
    elif [[ "$was_user_active" == true ]]; then
      run_user_service start easymanage-agent.service
    fi
    printf 'Migration failed; prior service/state restoration attempted. Recovery: %s\n' "$backup_dir" >&2
  fi
  exit "$result"
}
trap restore_on_exit EXIT
mutation_started=true
if [[ "$was_user_active" == true ]]; then run_user_service stop easymanage-agent.service; fi
systemctl stop easymanage-agent.service
if systemctl is-active --quiet easymanage-agent.service || run_user_service is-active --quiet easymanage-agent.service; then
  printf 'An EasyManage agent is still active; refusing to replace its SQLite state.\n' >&2
  exit 1
fi
# The active service is authoritative. A repeated handover preserves an already migrated user DB.
source_database=/var/lib/easymanage-agent/state.db
source_runner=()
if [[ "$was_root_active" != true && -f "$agent_base/state/state.db" ]] \
  && { [[ "$was_user_active" == true ]] || [[ "$was_root_enabled" == false ]]; }; then
  source_database="$agent_base/state/state.db"
  source_runner=(runuser -u "$operator_user" --)
fi
[[ -f "$source_database" ]] || { printf 'Existing agent identity database is missing; no new identity was created.\n' >&2; exit 1; }
# SQLite backup incorporates committed WAL records. Never combine DB/WAL components from different generations.
"${source_runner[@]}" python3 - "$source_database" "$expected_node" > "$backup_dir/identity.db" <<'PY'
import pathlib, shutil, sqlite3, sys, tempfile, time, uuid
from urllib.parse import quote
source, expected = sys.argv[1:]
deadline = time.monotonic() + 20
def progress(_status, _remaining, _total):
    if time.monotonic() > deadline:
        raise RuntimeError("SQLite identity snapshot timed out")
with sqlite3.connect("file:" + quote(source, safe="/") + "?mode=ro", uri=True, timeout=5) as old:
    row = old.execute("SELECT value FROM metadata WHERE key='node_id'").fetchone()
    if not row or (expected and uuid.UUID(row[0]) != uuid.UUID(expected)):
        raise RuntimeError("Persisted node identity does not match the deployment inventory")
    uuid.UUID(row[0])
    with tempfile.TemporaryDirectory(prefix="easymanage-identity-snapshot.") as temporary:
        destination = pathlib.Path(temporary) / "state.db"
        new = sqlite3.connect(destination)
        try:
            old.backup(new, pages=256, progress=progress, sleep=0.05)
            if new.execute("PRAGMA quick_check").fetchone()[0] != "ok":
                raise RuntimeError("SQLite identity snapshot integrity check failed")
        finally:
            new.close()
        with destination.open("rb") as snapshot:
            shutil.copyfileobj(snapshot, sys.stdout.buffer)
PY
run_operator install -d -m 0700 "$agent_base" "$agent_base/state"
for state_name in state.db state.db-wal state.db-shm; do
  if [[ -f "$agent_base/state/$state_name" ]]; then
    run_operator dd status=none if="$agent_base/state/$state_name" > "$backup_dir/previous-user-state/$state_name"
    previous_state_names+=("$state_name")
  fi
done
state_replaced=true
for state_name in "${previous_state_names[@]}"; do
  run_operator unlink "$agent_base/state/$state_name"
done
write_user_state "$agent_base/state/state.db" < "$backup_dir/identity.db"
systemctl disable easymanage-agent.service
loginctl enable-linger "$operator_user"
# Start the user's existing systemd manager; do not start the agent before its installer is ready.
systemctl start "user@$operator_uid.service"
trap - EXIT
printf 'Legacy Agent stopped; complete SQLite identity preserved. Recovery: %s\n' "$backup_dir"
printf 'Repeat ordinary-user deployment from Master to start basic mode.\n'
