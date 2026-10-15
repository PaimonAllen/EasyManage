#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
set -Eeuo pipefail

# Read-only inspection of the nginx port 80 site that EasyManage mirrors at
# /guide/ on the platform origin. Prints configuration, document root and
# HTTP checks. It changes no file and touches no service.

readonly nginx_sites_enabled="/etc/nginx/sites-enabled"
readonly nginx_conf_d="/etc/nginx/conf.d"
readonly check_ports="80 443 8443"

show_help() {
  printf '%s\n' \
    'Inspect the nginx port 80 site (EasyManage /guide/ source) read-only.' \
    '' \
    "Usage: $0" \
    '' \
    'Prints the effective nginx configuration, enabled sites, document' \
    'root contents and HTTP checks for local ports. No file, permission' \
    'or service is modified.' \
    'Run as root when config files are not readable by the current user.'
}

section() {
  printf '\n=== %s ===\n' "$1"
}

section 'Host / version'
hostname
date -Is
nginx -v 2>&1 || true

section 'Listening sockets'
ss -tlnp | grep -E ':(80|443|8443)\b' || printf 'No listener on ports %s\n' "$check_ports"

section 'Enabled sites'
if (( EUID != 0 )) && [ ! -r "$nginx_sites_enabled" ]; then
  printf 'Cannot list %s as current user; rerun as root.\n' "$nginx_sites_enabled"
else
  ls -la "$nginx_sites_enabled"
  printf '\nResolved targets:\n'
  for link in "$nginx_sites_enabled"/*; do
    [ -e "$link" ] || continue
    printf '%s -> %s\n' "$link" "$(readlink -f "$link")"
  done
  if [ -d "$nginx_conf_d" ] && [ -n "$(ls -A "$nginx_conf_d" 2>/dev/null)" ]; then
    printf '\nconf.d files:\n'
    ls -la "$nginx_conf_d"
  fi
fi

section 'Effective configuration (nginx -T)'
if command -v nginx >/dev/null 2>&1 && nginx -t >/dev/null 2>&1; then
  nginx -T 2>/dev/null || printf 'nginx -T failed; see per-file dumps below.\n' || true
else
  printf 'nginx -T unavailable (needs root and a valid config); dumping files directly.\n'
  for link in "$nginx_sites_enabled"/*; do
    [ -e "$link" ] || continue
    target="$(readlink -f "$link")"
    printf '\n--- %s ---\n' "$target"
    cat "$target" 2>&1 || true
  done
fi

section 'Document root'
doc_roots="$(nginx -T 2>/dev/null | grep -oE '^\s*root\s+[^;]+;' | awk '{print $2}' | tr -d ';' | sort -u || true)"
if [ -z "$doc_roots" ]; then
  printf 'Could not determine doc root from config (needs root).\n'
else
  for root_path in $doc_roots; do
    printf '%s\n' "$root_path"
    # Human-readable diagnostics only; filenames are never parsed or executed.
    # shellcheck disable=SC2012
    ls -la "$root_path" 2>&1 | head -30 || true
    if [ -f "$root_path/index.html" ]; then
      printf '\nindex.html title:\n'
      grep -oE '<title[^>]*>[^<]*</title>' "$root_path/index.html" | head -3 || true
      stat -c 'built: %y' "$root_path/index.html" || true
    fi
  done
fi

section 'HTTP checks (local)'
for url in http://127.0.0.1/ https://127.0.0.1:8443/guide/; do
  printf '%s\n' "$url"
  curl -skI --max-time 5 "$url" | head -8 || printf '  request failed\n'
  printf '\n'
done

section 'Done'
printf 'Read-only inspection finished; nothing was modified.\n'
