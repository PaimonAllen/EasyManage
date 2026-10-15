#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
set -Eeuo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
project_root="$(cd -- "$script_dir/../../.." && pwd)"
server_binary="$project_root/target/release/easymanage-server"
agent_binary="$project_root/target/release/easymanage-agent"
web_dist="$project_root/web/dist"
server_unit="$project_root/deploy/linux/system/easymanage-server.service"
agent_unit="$project_root/deploy/linux/system/easymanage-agent.service"
config_directory="/etc/easymanage"
server_environment="$config_directory/server.env"
agent_environment="$config_directory/agent.env"
tls_directory="$config_directory/tls"
server_address="${EASYMANAGE_SERVER_ADDRESS:-}"

if (( EUID != 0 )); then
  if ! command -v sudo >/dev/null 2>&1; then
    printf 'Administrator privileges are required and sudo is unavailable.\n' >&2
    exit 1
  fi
  exec sudo --preserve-env=EASYMANAGE_SERVER_ADDRESS bash "$0" "$@"
fi

for required in "$server_binary" "$agent_binary" "$web_dist/index.html" "$server_unit" "$agent_unit"; do
  if [[ ! -e "$required" ]]; then
    printf 'Required release artifact is missing: %s\n' "$required" >&2
    printf 'Build as the normal user first: cargo build --workspace --release && (cd web && pnpm build)\n' >&2
    exit 1
  fi
done
if [[ ! -r "$server_environment" ]]; then
  printf 'Missing %s; configure an existing PostgreSQL connection using server/config/server.env.example, or explicitly initialize a new local database.\n' "$server_environment" >&2
  exit 1
fi

if [[ -z "$server_address" ]]; then
  server_address="$(hostname -I | awk '{print $1}')"
fi
if [[ ! "$server_address" =~ ^[0-9a-fA-F:.]+$ ]]; then
  printf 'EASYMANAGE_SERVER_ADDRESS must be an IP address; received: %s\n' "$server_address" >&2
  exit 1
fi

# shellcheck disable=SC1090
source "$server_environment"
[[ -n "${DATABASE_URL:-}" || -n "${PGHOST:-}" ]] || {
  printf 'Configure DATABASE_URL or PG* variables in server.env.\n' >&2; exit 1;
}
enrollment_token="${EASYMANAGE_ENROLLMENT_TOKEN:-$(openssl rand -hex 32)}"

install -d -o root -g root -m 0700 "$config_directory" "$tls_directory"
umask 0077
environment_temp="$(mktemp "$config_directory/server.env.XXXXXX")"
cp -- "$server_environment" "$environment_temp"
# Preserve the original PostgreSQL variables and all custom platform settings;
# append only missing system-layout defaults. Never interpolate a password.
while IFS= read -r setting; do
  key="${setting%%=*}"
  if ! grep -q "^${key}=" "$environment_temp"; then printf '%s\n' "$setting" >> "$environment_temp"; fi
done < <(printf '%s\n' \
  "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" \
  'EASYMANAGE_BIND=0.0.0.0:8443' \
  'EASYMANAGE_TLS_CERTIFICATE=/etc/easymanage/tls/server.crt' \
  'EASYMANAGE_TLS_PRIVATE_KEY=/etc/easymanage/tls/server.key' \
  'EASYMANAGE_WEB_ROOT=/opt/easymanage/web' \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' \
  'EASYMANAGE_ONLINE_WINDOW_SECONDS=45' \
  'EASYMANAGE_METRICS_RETENTION_DAYS=30' \
  )
install -o root -g root -m 0600 "$environment_temp" "$server_environment"
rm -f -- "$environment_temp"

ca_key="$tls_directory/ca.key"
ca_certificate="$tls_directory/ca.crt"
server_key="$tls_directory/server.key"
server_certificate="$tls_directory/server.crt"
if [[ ! -s "$ca_key" || ! -s "$ca_certificate" || ! -s "$server_key" || ! -s "$server_certificate" ]]; then
  if [[ -e "$ca_key" || -e "$ca_certificate" || -e "$server_key" || -e "$server_certificate" ]]; then
    printf 'Incomplete TLS material exists in %s; refusing to overwrite it.\n' "$tls_directory" >&2
    exit 1
  fi

  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$ca_key"
  openssl req -x509 -new -sha256 -days 3650 \
    -key "$ca_key" -out "$ca_certificate" -subj '/CN=EasyManage Local Root CA' \
    -addext 'basicConstraints=critical,CA:TRUE' \
    -addext 'keyUsage=critical,keyCertSign,cRLSign'
  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:3072 -out "$server_key"
  certificate_request="$(mktemp "$tls_directory/server.csr.XXXXXX")"
  extension_file="$(mktemp "$tls_directory/server.ext.XXXXXX")"
  host_name="$(hostname)"
  printf '%s\n' \
    '[v3_server]' \
    'basicConstraints=critical,CA:FALSE' \
    'keyUsage=critical,digitalSignature,keyEncipherment' \
    'extendedKeyUsage=serverAuth' \
    "subjectAltName=DNS:${host_name},DNS:localhost,IP:127.0.0.1,IP:${server_address}" \
    > "$extension_file"
  openssl req -new -sha256 -key "$server_key" -out "$certificate_request" \
    -subj "/CN=${host_name}"
  openssl x509 -req -sha256 -days 825 \
    -in "$certificate_request" -CA "$ca_certificate" -CAkey "$ca_key" -CAcreateserial \
    -out "$server_certificate" -extfile "$extension_file" -extensions v3_server
  rm -f -- "$certificate_request" "$extension_file" "$tls_directory/ca.srl"
fi
chmod 0600 "$ca_key" "$server_key"
chmod 0644 "$ca_certificate" "$server_certificate"
openssl verify -CAfile "$ca_certificate" "$server_certificate"

install -o root -g root -m 0755 "$server_binary" /usr/local/sbin/easymanage-server
install -o root -g root -m 0755 "$agent_binary" /usr/local/sbin/easymanage-agent
install -d -o root -g root -m 0755 /opt/easymanage/web
cp -a -- "$web_dist/." /opt/easymanage/web/
chown -R root:root /opt/easymanage/web
install -d -o root -g root -m 0700 /var/lib/easymanage-server /var/lib/easymanage-agent
install -o root -g root -m 0644 "$server_unit" /etc/systemd/system/easymanage-server.service
install -o root -g root -m 0644 "$agent_unit" /etc/systemd/system/easymanage-agent.service

agent_environment_temp="$(mktemp "$config_directory/agent.env.XXXXXX")"
printf '%s\n' \
  "EASYMANAGE_SERVER_URL=https://${server_address}:8443" \
  'EASYMANAGE_CA_CERTIFICATE=/etc/easymanage/tls/ca.crt' \
  'EASYMANAGE_AGENT_STATE_DB=/var/lib/easymanage-agent/state.db' \
  "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' \
  > "$agent_environment_temp"
install -o root -g root -m 0600 "$agent_environment_temp" "$agent_environment"
rm -f -- "$agent_environment_temp"

systemctl daemon-reload
systemctl enable easymanage-server.service easymanage-agent.service
systemctl restart easymanage-server.service easymanage-agent.service
systemctl --no-pager --full status easymanage-server.service easymanage-agent.service || true

printf '\nEasyManage server and local agent are installed.\n'
printf 'Dashboard URL: https://%s:8443/\n' "$server_address"
