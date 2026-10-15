#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
set -Eeuo pipefail

readonly postgres_major="${EASYMANAGE_POSTGRES_MAJOR:-18}"
readonly database_name="${EASYMANAGE_DATABASE_NAME:-easymanage}"
readonly database_role="${EASYMANAGE_DATABASE_ROLE:-easymanage}"
readonly requested_port="${EASYMANAGE_POSTGRES_PORT:-}"
readonly config_directory="/etc/easymanage"
readonly secret_file="$config_directory/server.env"

show_help() {
  printf '%s\n' \
    'Install a configurable PostgreSQL version beside existing clusters and initialize EasyManage.' \
    '' \
    "Usage: $0 --apply [--rotate-password]" \
    '' \
    'Options:' \
    '  --apply            Approve package, database and root configuration changes.' \
    '  --rotate-password  Generate a new database password and update server.env.' \
    'Configuration: EASYMANAGE_POSTGRES_MAJOR (default 18), EASYMANAGE_POSTGRES_PORT (auto),'
  printf '%s\n' \
    '  EASYMANAGE_DATABASE_NAME and EASYMANAGE_DATABASE_ROLE (default easymanage).' \
    '  -h, --help         Show this help.'
}

rotate_password="false"
apply="false"
for argument in "$@"; do
  case "$argument" in
    --apply) apply="true" ;;
    --rotate-password) rotate_password="true" ;;
    -h|--help) show_help; exit 0 ;;
    *) printf 'Unknown option: %s\n' "$argument" >&2; exit 64 ;;
  esac
done
[[ "$apply" == true ]] || { show_help; printf 'No changes made; --apply is required.\n' >&2; exit 64; }
[[ "$postgres_major" =~ ^[0-9]{2}$ && "$postgres_major" -ge 14 ]] || { printf 'Invalid PostgreSQL major version.\n' >&2; exit 64; }
[[ "$database_name" =~ ^[a-z_][a-z0-9_]{0,62}$ && "$database_role" =~ ^[a-z_][a-z0-9_]{0,62}$ ]] || {
  printf 'Database and role must be lowercase SQL identifiers, at most 63 characters.\n' >&2; exit 64;
}
[[ -z "$requested_port" || "$requested_port" =~ ^[1-9][0-9]{0,4}$ && "$requested_port" -le 65535 ]] || {
  printf 'Invalid PostgreSQL port.\n' >&2; exit 64;
}

if [[ ! -r /etc/os-release ]]; then
  printf 'Cannot identify this operating system.\n' >&2
  exit 1
fi

# shellcheck disable=SC1091
source /etc/os-release
if [[ "${ID:-}" != "ubuntu" ]]; then
  printf 'This script supports Ubuntu only; detected: %s\n' "${ID:-unknown}" >&2
  exit 1
fi

if (( EUID != 0 )); then
  if ! command -v sudo >/dev/null 2>&1; then
    printf 'Administrator privileges are required and sudo is unavailable.\n' >&2
    exit 1
  fi
  exec sudo env EASYMANAGE_POSTGRES_MAJOR="$postgres_major" EASYMANAGE_POSTGRES_PORT="$requested_port" \
    EASYMANAGE_DATABASE_NAME="$database_name" EASYMANAGE_DATABASE_ROLE="$database_role" bash "$0" "$@"
fi

if [[ -z "${VERSION_CODENAME:-}" ]]; then
  printf 'Ubuntu VERSION_CODENAME is unavailable.\n' >&2
  exit 1
fi

system_architecture="$(dpkg --print-architecture)"
pgdg_key_directory="/usr/share/postgresql-common/pgdg"
pgdg_key_file="$pgdg_key_directory/apt.postgresql.org.asc"
pgdg_source_file="/etc/apt/sources.list.d/pgdg.sources"

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install --yes ca-certificates curl postgresql-common

install -d -m 0755 "$pgdg_key_directory"
curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
  --output "$pgdg_key_file" \
  https://www.postgresql.org/media/keys/ACCC4CF8.asc
chmod 0644 "$pgdg_key_file"

printf '%s\n' \
  'Types: deb' \
  'URIs: https://apt.postgresql.org/pub/repos/apt' \
  "Suites: ${VERSION_CODENAME}-pgdg" \
  "Architectures: $system_architecture" \
  'Components: main' \
  "Signed-By: $pgdg_key_file" \
  > "$pgdg_source_file"
chmod 0644 "$pgdg_source_file"

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install --yes \
  "postgresql-$postgres_major" \
  "postgresql-client-$postgres_major"

cluster_summary="$(
  pg_lsclusters --no-header \
    | awk -v target_version="$postgres_major" \
      '$1 == target_version { print $1, $2, $3, $4; exit }'
)"

if [[ -z "$cluster_summary" ]]; then
  port_options=()
  [[ -z "$requested_port" ]] || port_options=(--port "$requested_port")
  pg_createcluster "$postgres_major" main "${port_options[@]}" --start
  cluster_summary="$(
    pg_lsclusters --no-header \
      | awk -v target_version="$postgres_major" \
        '$1 == target_version { print $1, $2, $3, $4; exit }'
  )"
fi

read -r cluster_version cluster_name cluster_port cluster_status <<< "$cluster_summary"
if [[ -n "$requested_port" && "$requested_port" != "$cluster_port" ]]; then
  printf 'Existing cluster is on port %s, not requested port %s; refusing to reconfigure it.\n' "$cluster_port" "$requested_port" >&2
  exit 1
fi
if [[ "$cluster_status" != "online" ]]; then
  pg_ctlcluster "$cluster_version" "$cluster_name" start
fi
systemctl enable postgresql

database_password=""
enrollment_token=""
if [[ -r "$secret_file" ]]; then
  # This root-owned file is created by this script and contains only fixed variable names.
  # shellcheck disable=SC1090
  source "$secret_file"
  database_password="${EASYMANAGE_DATABASE_PASSWORD:-}"
  enrollment_token="${EASYMANAGE_ENROLLMENT_TOKEN:-}"
fi
if [[ "$rotate_password" == "true" ]]; then database_password=""; fi

if [[ -z "$database_password" ]]; then
  database_password="$(openssl rand -hex 32)"
fi
if [[ -z "$enrollment_token" ]]; then
  enrollment_token="$(openssl rand -hex 32)"
fi
[[ "$database_password" =~ ^[a-fA-F0-9]{64}$ ]] || {
  printf 'This initializer only accepts its generated hex password; use general PG* configuration for an existing database.\n' >&2; exit 1;
}

role_exists="$(
  runuser -u postgres -- psql \
    --port="$cluster_port" \
    --dbname=postgres \
    --no-psqlrc \
    --tuples-only \
    --no-align \
    --command="SELECT 1 FROM pg_roles WHERE rolname = '$database_role';"
)"

if [[ "$role_exists" == "1" ]]; then
  printf \
    "ALTER ROLE \"%s\" WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD '%s';\n" \
    "$database_role" "$database_password" \
    | runuser -u postgres -- psql \
      --port="$cluster_port" \
      --dbname=postgres \
      --no-psqlrc \
      --set=ON_ERROR_STOP=1
else
  printf \
    "CREATE ROLE \"%s\" WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD '%s';\n" \
    "$database_role" "$database_password" \
    | runuser -u postgres -- psql \
      --port="$cluster_port" \
      --dbname=postgres \
      --no-psqlrc \
      --set=ON_ERROR_STOP=1
fi

database_exists="$(
  runuser -u postgres -- psql \
    --port="$cluster_port" \
    --dbname=postgres \
    --no-psqlrc \
    --tuples-only \
    --no-align \
    --command="SELECT 1 FROM pg_database WHERE datname = '$database_name';"
)"

if [[ "$database_exists" != "1" ]]; then
  runuser -u postgres -- createdb \
    --port="$cluster_port" \
    --owner="$database_role" \
    --encoding=UTF8 \
    --template=template0 \
    "$database_name"
fi

printf '%s\n' \
  "REVOKE ALL ON DATABASE \"$database_name\" FROM PUBLIC;" \
  "GRANT CONNECT, TEMPORARY ON DATABASE \"$database_name\" TO \"$database_role\";" \
  | runuser -u postgres -- psql \
    --port="$cluster_port" \
    --dbname=postgres \
    --no-psqlrc \
    --set=ON_ERROR_STOP=1

printf '%s\n' \
  'REVOKE CREATE ON SCHEMA public FROM PUBLIC;' \
  "GRANT USAGE, CREATE ON SCHEMA public TO \"$database_role\";" \
  | runuser -u postgres -- psql \
    --port="$cluster_port" \
    --dbname="$database_name" \
    --no-psqlrc \
    --set=ON_ERROR_STOP=1

install -d -m 0700 "$config_directory"
umask 0077
printf '%s\n' \
  "EASYMANAGE_DATABASE_PASSWORD=$database_password" \
  "DATABASE_URL=postgresql://${database_role}:${database_password}@127.0.0.1:${cluster_port}/${database_name}" \
  "EASYMANAGE_ENROLLMENT_TOKEN=$enrollment_token" \
  'EASYMANAGE_BIND=0.0.0.0:8443' \
  'EASYMANAGE_TLS_CERTIFICATE=/etc/easymanage/tls/server.crt' \
  'EASYMANAGE_TLS_PRIVATE_KEY=/etc/easymanage/tls/server.key' \
  'EASYMANAGE_WEB_ROOT=/opt/easymanage/web' \
  'EASYMANAGE_REPORT_INTERVAL_SECONDS=10' \
  'EASYMANAGE_ONLINE_WINDOW_SECONDS=45' \
  'EASYMANAGE_METRICS_RETENTION_DAYS=30' \
  > "$secret_file"
chmod 0600 "$secret_file"

PGPASSWORD="$database_password" psql \
  --host=127.0.0.1 \
  --port="$cluster_port" \
  --username="$database_role" \
  --dbname="$database_name" \
  --no-psqlrc \
  --pset=pager=off \
  --set=ON_ERROR_STOP=1 \
  --command='SELECT current_user, current_database(), version();'

printf '\nEasyManage database initialization completed.\n'
printf 'PostgreSQL cluster: %s/%s\n' "$cluster_version" "$cluster_name"
printf 'PostgreSQL port: %s\n' "$cluster_port"
printf 'Database: %s\n' "$database_name"
printf 'Role: %s\n' "$database_role"
printf 'Root-only configuration: %s\n' "$secret_file"
printf 'Existing PostgreSQL clusters were preserved.\n'
