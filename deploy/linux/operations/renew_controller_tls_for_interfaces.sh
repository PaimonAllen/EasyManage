#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# Reissue only the controller's leaf certificate for IPs currently assigned to this host.
# This is an explicit maintenance action; it never runs during normal deployment.
set -Eeuo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
umask 0077

if [[ $EUID != 0 || ${1:-} != --apply || $# != 1 ]]; then
  printf 'Run as root: EASYMANAGE_OPERATOR_USER=operator bash %s --apply\n' "$0" >&2
  printf 'This replaces the controller leaf certificate and briefly restarts its user service.\n' >&2
  exit 64
fi

operator_user="${EASYMANAGE_OPERATOR_USER:-}"
[[ "$operator_user" =~ ^[a-z_][a-z0-9_-]*$ && "$operator_user" != root ]] || {
  printf 'Set EASYMANAGE_OPERATOR_USER to the ordinary controller service user.\n' >&2
  exit 64
}
operator_home="$(getent passwd "$operator_user" | cut -d: -f6)"
operator_uid="$(id -u "$operator_user")"
operator_gid="$(id -g "$operator_user")"
[[ "$operator_home" =~ ^/home/[a-zA-Z0-9_.-]+$ && ! -L "$operator_home" ]] || exit 64
user_config="$operator_home/.easymanage/config"
[[ -d "$user_config" && ! -L "$user_config" ]] || exit 64
for file in "$user_config/server.crt" "$user_config/server.key" "$user_config/ca.crt"; do
  [[ -f "$file" && ! -L "$file" ]] || { printf 'Missing or unsafe file: %s\n' "$file" >&2; exit 1; }
done
grep -Fqx "EASYMANAGE_TLS_CERTIFICATE=$user_config/server.crt" "$user_config/server.env" || {
  printf 'Controller is not configured to use the certificate this script would replace.\n' >&2
  exit 1
}
ca_key=/etc/easymanage/tls/ca.key
ca_certificate=/etc/easymanage/tls/ca.crt
[[ -f "$ca_key" && ! -L "$ca_key" && -f "$ca_certificate" && ! -L "$ca_certificate" ]] || {
  printf 'Existing root CA material is unavailable; refusing to create a different CA.\n' >&2
  exit 1
}
cmp -s "$ca_certificate" "$user_config/ca.crt" || {
  printf 'Controller and root CA certificates differ; refusing to sign.\n' >&2
  exit 1
}
run_operator() {
  runuser -u "$operator_user" -- env XDG_RUNTIME_DIR="/run/user/$operator_uid" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$operator_uid/bus" "$@"
}
run_operator systemctl --user is-active --quiet easymanage-server.service || {
  printf 'The ordinary-user controller service is not active; no certificate changed.\n' >&2
  exit 1
}

work_dir="$(mktemp -d /etc/easymanage/tls/interface-renew.XXXXXXXX)"
changed=false
backup=""
cleanup() {
  result=$?
  trap - EXIT HUP INT TERM
  if [[ "$result" != 0 && "$changed" == true && -n "$backup" ]]; then
    rollback_certificate="$(mktemp "$user_config/server.crt.rollback.XXXXXXXX")"
    install -o "$operator_uid" -g "$operator_gid" -m 0600 "$backup" "$rollback_certificate"
    mv -Tf -- "$rollback_certificate" "$user_config/server.crt"
    run_operator systemctl --user restart easymanage-server.service || true
    printf 'Certificate update failed; previous certificate restored. Check service health.\n' >&2
  fi
  rm -r -- "$work_dir"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM

python3 - "$work_dir/server.ext" "$work_dir/addresses" <<'PY'
import ipaddress
import json
import re
import socket
import subprocess
import sys

hostname = socket.gethostname()
if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.-]*", hostname):
    raise SystemExit("Unsafe hostname; certificate was not changed")
interfaces = json.loads(subprocess.check_output(["ip", "-j", "address", "show"], text=True))
addresses = {ipaddress.ip_address("127.0.0.1"), ipaddress.ip_address("::1")}
for interface in interfaces:
    for info in interface.get("addr_info", []):
        if info.get("family") in ("inet", "inet6"):
            addresses.add(ipaddress.ip_address(info["local"].split("%", 1)[0]))
ordered = sorted(addresses, key=lambda value: (value.version, int(value)))
with open(sys.argv[1], "w", encoding="ascii") as extension:
    extension.write("[v3_server]\n")
    extension.write("basicConstraints=critical,CA:FALSE\n")
    extension.write("keyUsage=critical,digitalSignature,keyEncipherment\n")
    extension.write("extendedKeyUsage=serverAuth\n")
    extension.write("subjectAltName=DNS:" + hostname + ",DNS:localhost," +
                    ",".join("IP:" + str(address) for address in ordered) + "\n")
with open(sys.argv[2], "w", encoding="ascii") as output:
    output.writelines(str(address) + "\n" for address in ordered)
PY

printf 'Controller certificate will cover these current local interface IPs:\n'
sed 's/^/  /' "$work_dir/addresses"
already_covered=true
while IFS= read -r address; do
  if ! openssl verify -CAfile "$ca_certificate" -verify_ip "$address" \
    "$user_config/server.crt" >/dev/null 2>&1; then
    already_covered=false
    break
  fi
done < "$work_dir/addresses"
if [[ "$already_covered" == true ]] && openssl verify -CAfile "$ca_certificate" \
  -verify_hostname "$(hostname)" "$user_config/server.crt" >/dev/null 2>&1; then
  printf 'Existing certificate already covers every current interface IP; no change needed.\n'
  exit 0
fi
openssl req -new -sha256 -key "$user_config/server.key" -out "$work_dir/server.csr" \
  -subj "/CN=$(hostname)"
openssl x509 -req -sha256 -days 825 -in "$work_dir/server.csr" \
  -CA "$ca_certificate" -CAkey "$ca_key" -set_serial "0x$(openssl rand -hex 16)" \
  -out "$work_dir/server.crt" -extfile "$work_dir/server.ext" -extensions v3_server
openssl verify -CAfile "$ca_certificate" -purpose sslserver "$work_dir/server.crt"
while IFS= read -r address; do
  openssl verify -CAfile "$ca_certificate" -verify_ip "$address" "$work_dir/server.crt" >/dev/null
done < "$work_dir/addresses"
openssl verify -CAfile "$ca_certificate" -verify_hostname "$(hostname)" "$work_dir/server.crt" >/dev/null
cert_key_hash="$(openssl x509 -in "$work_dir/server.crt" -pubkey -noout | openssl sha256)"
private_key_hash="$(openssl pkey -in "$user_config/server.key" -pubout | openssl sha256)"
[[ "$cert_key_hash" == "$private_key_hash" ]] || { printf 'Certificate and private key mismatch.\n' >&2; exit 1; }

backup="$user_config/server.crt.before-interface-renew.$(date -u +%Y%m%dT%H%M%SZ)"
[[ ! -e "$backup" ]] || { printf 'Backup already exists: %s\n' "$backup" >&2; exit 1; }
cp -p -- "$user_config/server.crt" "$backup"
next_certificate="$(mktemp "$user_config/server.crt.next.XXXXXXXX")"
install -o "$operator_uid" -g "$operator_gid" -m 0600 "$work_dir/server.crt" "$next_certificate"
mv -Tf -- "$next_certificate" "$user_config/server.crt"
changed=true
run_operator systemctl --user restart easymanage-server.service
ready=false
for _ in {1..15}; do
  if run_operator systemctl --user is-active --quiet easymanage-server.service && \
    curl --noproxy '*' --fail --silent --max-time 2 --cacert "$user_config/ca.crt" \
      https://127.0.0.1:8443/api/v1/health/ready >/dev/null; then
    ready=true
    break
  fi
  sleep 1
done
[[ "$ready" == true ]] || { printf 'Controller failed readiness after certificate renewal.\n' >&2; exit 1; }
changed=false
printf 'Controller certificate updated; backup: %s\n' "$backup"
printf 'Clients must trust the existing EasyManage CA. Rerun after interface IP changes.\n'
