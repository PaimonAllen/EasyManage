#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# Deploy this repository's web/dist to an existing user platform.
set -Eeuo pipefail
[[ $EUID != 0 ]] || { printf 'Run as the existing platform owner, not root.\n' >&2; exit 1; }
operator_user="$(id -un)"
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
bundle="$script_dir/../../../web/dist"
user_base="${HOME:?}/.local/share/easymanage"
destination="$user_base/web"
ca="$user_base/config/ca.crt"
url="${EASYMANAGE_PLATFORM_URL:-https://127.0.0.1:8443}"
[[ "$url" == https://* ]] || { printf 'EASYMANAGE_PLATFORM_URL must use HTTPS.\n' >&2; exit 64; }
[[ -f "$bundle/index.html" && -d "$bundle/assets" && -d "$destination" && ! -L "$bundle" && ! -L "$destination" ]]
[[ "$(stat -c %U "$script_dir")" == "$operator_user" && "$(stat -c %U "$bundle")" == "$operator_user" ]]
[[ -z "$(find "$bundle" -type l -print -quit)" ]] || {
  printf 'Refusing a bundle containing symlinks.\n' >&2; exit 1;
}
rg -q -F -x "EASYMANAGE_WEB_ROOT=$destination" "$user_base/config/server.env"
systemctl --user is-active --quiet easymanage-server.service
before_pid="$(systemctl --user show easymanage-server.service -p MainPID --value)"
curl --noproxy '*' --cacert "$ca" -fsS --max-time 5 "$url/api/v1/health/ready" >/dev/null
while IFS= read -r asset; do
  [[ -f "$bundle$asset" ]] || { printf 'Missing referenced asset: %s\n' "$asset" >&2; exit 1; }
done < <(rg -o '/assets/[^" ]+' "$bundle/index.html")

backup="$(mktemp -d "$user_base/web-backup.XXXXXX")"
declare -a replaced=()
atomic_copy() {
  local source="$1" target="$2" temporary
  [[ ! -L "$target" ]] || { printf 'Refusing symlink target: %s\n' "$target" >&2; return 1; }
  mkdir -p -- "$(dirname -- "$target")"
  temporary="$(mktemp "$(dirname -- "$target")/.web-deploy.XXXXXX")"
  cp -- "$source" "$temporary"
  chmod 600 "$temporary"
  mv -T -- "$temporary" "$target"
}
rollback() {
  local status="$?" relative
  trap - ERR
  set +e
  for relative in "${replaced[@]}"; do
    if [[ -f "$backup/$relative" ]]; then
      atomic_copy "$backup/$relative" "$destination/$relative"
    fi
  done
  printf 'Deployment failed; previous files restored where present. New hashed assets retained. Backup: %s\n' "$backup" >&2
  exit "$status"
}
trap rollback ERR
install_file() {
  local source="$1" relative="${1#"$bundle/"}"
  [[ ! -L "$destination/$relative" ]]
  if [[ -f "$destination/$relative" ]]; then
    if cmp -s -- "$source" "$destination/$relative"; then return; fi
    mkdir -p -- "$(dirname -- "$backup/$relative")"
    cp -p -- "$destination/$relative" "$backup/$relative"
  fi
  replaced+=("$relative")
  atomic_copy "$source" "$destination/$relative"
}
# Keep old hashes for existing browser tabs. Publish new dependencies before HTML.
while IFS= read -r -d '' source; do
  [[ "$source" == "$bundle/index.html" ]] || install_file "$source"
done < <(find "$bundle" -type f -print0)
install_file "$bundle/index.html"

curl --noproxy '*' --cacert "$ca" -fsS --max-time 5 "$url/" | cmp -s - "$bundle/index.html"
while IFS= read -r asset; do
  curl --noproxy '*' --cacert "$ca" -fsS --max-time 5 "$url$asset" | cmp -s - "$bundle$asset"
done < <(rg -o '/assets/[^" ]+' "$bundle/index.html")
curl --noproxy '*' --cacert "$ca" -fsS --max-time 5 "$url/api/v1/health/ready" >/dev/null
[[ "$(systemctl --user show easymanage-server.service -p MainPID --value)" == "$before_pid" ]]
trap - ERR
printf 'Frontend deployed without restarting services. Controller PID: %s\nRollback files: %s\n' "$before_pid" "$backup"
