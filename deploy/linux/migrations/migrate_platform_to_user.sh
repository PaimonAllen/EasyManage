#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# shellcheck disable=SC2016
# One administrator handover; future launch/deploy uses scripts/run-server.sh --fleet.
set -Eeuo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 0077
project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)"
operator_user="${EASYMANAGE_OPERATOR_USER:-$(stat -c %U "$project_root")}"
[[ $EUID == 0 && "$operator_user" =~ ^[a-z_][a-z0-9_-]*$ && "$operator_user" != root ]] || {
  printf 'Run as root with EASYMANAGE_OPERATOR_USER set to the ordinary SSH user.\n' >&2; exit 64;
}
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
operator_uid="$(id -u "$operator_user")"
user_base="$operator_home/.local/share/easymanage"
[[ "$operator_home" =~ ^/home/[a-zA-Z0-9_.-]+$ && ! -L "$operator_home" && ! -L "$user_base" ]] || exit 64
run_operator() {
  runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" "$@"
}
for file in /etc/easymanage/server.env /etc/easymanage/tls/ca.crt \
  /etc/easymanage/tls/server.crt /etc/easymanage/tls/server.key \
  "$project_root/target/release/easymanage-server" "$project_root/web/dist/index.html" \
  "$user_base/config/staging.json" "$user_base/config/targets.json"; do
  [[ -f "$file" ]] || { printf 'Required handover input missing: %s\n' "$file" >&2; exit 1; }
done
local_bundle="$(/usr/bin/jq -er '.nodes[] | select(.ssh_target == "local") | .staging_directory' "$user_base/config/staging.json")"
[[ "$local_bundle" =~ ^/tmp/easymanage-agent\.[a-zA-Z0-9_-]+$ && -f "$local_bundle/handover.sh" ]] || exit 64
if ! systemctl is-active --quiet easymanage-server.service && \
  run_operator systemctl --user is-active --quiet easymanage-server.service; then
  printf 'Controller is already running as the ordinary user; only completing Agent handover.\n'
  exec bash "$local_bundle/handover.sh" "$operator_user"
fi
database_url="$(sed -n 's/^DATABASE_URL=//p' /etc/easymanage/server.env)"
enrollment_token="$(sed -n 's/^EASYMANAGE_ENROLLMENT_TOKEN=//p' /etc/easymanage/server.env)"
[[ "$database_url" == postgresql://* && "$enrollment_token" =~ ^[a-fA-F0-9]{64}$ ]] || exit 1
run_operator install -d -m 0700 \
  "$user_base" "$user_base/config" "$user_base/bin" "$user_base/web" "$user_base/state" \
  "$operator_home/.config/systemd/user"
for cert in ca.crt server.crt server.key; do
  # All writes inside the user's home run as that user, never as root.
  run_operator bash -c 'set -e; umask 077; temporary=$(mktemp "$1.next.XXXXXX"); trap '\''unlink "$temporary"'\'' EXIT; dd status=none of="$temporary"; mv -T "$temporary" "$1"; trap - EXIT' \
    bash "$user_base/config/$cert" < "/etc/easymanage/tls/$cert"
done
ca_hash="$(sha256sum "$user_base/config/ca.crt" | cut -d' ' -f1)"
run_operator bash -c 'set -e; temporary=$(mktemp "$1.next.XXXXXX"); trap '\''unlink "$temporary"'\'' EXIT; jq --arg hash "$2" '\''.targets |= map(.ca_sha256 = $hash)'\'' "$1" > "$temporary"; mv -T "$temporary" "$1"; trap - EXIT' \
  bash "$user_base/config/targets.json" "$ca_hash"
printf '%s\n' "DATABASE_URL=$database_url" "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" \
  'EASYMANAGE_BIND=0.0.0.0:8443' \
  "EASYMANAGE_TLS_CERTIFICATE=$user_base/config/server.crt" \
  "EASYMANAGE_TLS_PRIVATE_KEY=$user_base/config/server.key" \
  "EASYMANAGE_WEB_ROOT=$user_base/web" \
  "EASYMANAGE_OPERATIONS_TARGETS=$user_base/config/targets.json" \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' 'EASYMANAGE_ONLINE_WINDOW_SECONDS=45' \
  'EASYMANAGE_METRICS_RETENTION_DAYS=30' | \
  run_operator bash -c 'set -e; umask 077; temporary=$(mktemp "$1.next.XXXXXX"); trap '\''unlink "$temporary"'\'' EXIT; dd status=none of="$temporary"; mv -T "$temporary" "$1"; trap - EXIT' bash "$user_base/config/server.env"
run_operator install -m 0700 \
  "$project_root/target/release/easymanage-server" "$user_base/bin/easymanage-server"
run_operator cp -a "$project_root/web/dist/." "$user_base/web/"
run_operator install -m 0600 \
  "$project_root/deploy/linux/user/easymanage-server.user.service" \
  "$operator_home/.config/systemd/user/easymanage-server.service"
loginctl enable-linger "$operator_user"
previous_active=false
if systemctl is-active --quiet easymanage-server.service; then previous_active=true; fi
rollback() {
  result=$?
  trap - EXIT HUP INT TERM
  [[ "$result" != 0 ]] || return 0
  run_operator systemctl --user disable --now easymanage-server.service || true
  if [[ "$previous_active" == true ]]; then systemctl start easymanage-server.service || true; fi
  printf 'Controller handover failed; previous system controller restarted.\n' >&2
  exit "$result"
}
trap rollback EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
systemctl stop easymanage-server.service
run_operator systemctl --user daemon-reload
run_operator systemctl --user enable easymanage-server.service
run_operator systemctl --user restart easymanage-server.service
ready=false
for _ in {1..20}; do
  if run_operator systemctl --user is-active --quiet easymanage-server.service && \
    curl --noproxy '*' --fail --silent --max-time 2 \
    --cacert "$user_base/config/ca.crt" https://127.0.0.1:8443/api/v1/health/ready >/dev/null; then
    ready=true; break
  fi
  sleep 1
done
[[ "$ready" == true ]]
systemctl disable easymanage-server.service
trap - EXIT HUP INT TERM
bash "$local_bundle/handover.sh" "$operator_user"
printf 'Controller and local Agent now run as %s. Existing PostgreSQL and Nginx were retained.\n' "$operator_user"
printf 'The server 2 administrator must run the handover command printed by --prepare-fleet.\n'
