#!/usr/bin/env bash
# Embedded by the controller. Only fixed, validated account operations run here.
set -Eeuo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
umask 0077
[[ $EUID == 0 && $# == 7 ]] || exit 64
action="$1" actor="$2" subject="$3" display="$4" shell="$5" groups="$6" confirmation="$7"
[[ "$actor" =~ ^[a-z_][a-z0-9_-]{0,31}$ && "$subject" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || exit 64
[[ "$display" != *$'\n'* && "$display" != *:* && ${#display} -le 100 ]] || exit 64
[[ -z "$shell" || "$shell" == /bin/bash || "$shell" == /usr/bin/zsh || "$shell" == /bin/zsh || "$shell" == /usr/sbin/nologin ]] || exit 64
minimum_uid="$(awk '$1=="UID_MIN" {print $2; exit}' /etc/login.defs)"
[[ "$minimum_uid" =~ ^[0-9]+$ ]] || minimum_uid=1000
maximum_uid="$(awk '$1=="UID_MAX" {print $2; exit}' /etc/login.defs)"
[[ "$maximum_uid" =~ ^[0-9]+$ ]] || maximum_uid=60000
# Use grep on target machines where ripgrep is not normally installed.
is_administrator() {
  [[ "$1" == root ]] && return 0
  id -nG "$1" 2>/dev/null | tr ' ' '\n' | grep -Eq '^(sudo|lab-admins|wheel)$'
}
is_administrator "$actor" || { printf 'SSH account is not a delegated administrator\n' >&2; exit 77; }
if [[ "$action" == list ]]; then
  python3 - "$minimum_uid" "$maximum_uid" <<'PY'
import json, os, pwd, sys
low, high = map(int, sys.argv[1:])
accounts = []
for entry in pwd.getpwall():
    if entry.pw_uid != 0 and not low <= entry.pw_uid <= high:
        continue
    if os.path.basename(entry.pw_shell) in ("nologin", "false"):
        status = "restricted"
    elif os.path.isabs(entry.pw_shell) and os.path.isfile(entry.pw_shell) and os.access(entry.pw_shell, os.X_OK):
        status = "normal"
    else:
        status = "invalid"
    accounts.append({"username": entry.pw_name, "uid": entry.pw_uid,
                     "gid": entry.pw_gid, "home": entry.pw_dir,
                     "shell": entry.pw_shell, "display_name": entry.pw_gecos,
                     "account_status": status})
print(json.dumps({"accounts": accounts[:4096]}, ensure_ascii=False))
PY
  exit 0
fi
[[ "$actor" != "$subject" && "$subject" != root && "$subject" != easymanage ]] || exit 77
if [[ -n "$groups" && "$groups" != __keep__ ]]; then
  IFS=, read -ra selected <<< "$groups"
  (( ${#selected[@]} <= 16 )) || exit 64
  for group in "${selected[@]}"; do
    [[ "$group" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]] || exit 64
    case "$group" in
      root|sudo|wheel|adm|lab-admins|docker|lxd|disk|shadow|systemd-journal) exit 77 ;;
    esac
    getent group "$group" >/dev/null || exit 64
  done
fi
if [[ "$action" == create ]]; then
  getent passwd "$subject" >/dev/null && exit 73
  [[ "$confirmation" == "$subject" ]] || exit 64
  options=(-m -s "${shell:-/bin/bash}" -c "$display")
  [[ -z "$groups" ]] || options+=(-G "$groups")
  useradd "${options[@]}" "$subject"
  exit 0
fi
entry="$(getent passwd "$subject")"
[[ -n "$entry" ]] || exit 67
uid="$(printf '%s' "$entry" | cut -d: -f3)"
[[ "$uid" =~ ^[0-9]+$ ]] && (( uid >= minimum_uid && uid <= maximum_uid )) || exit 77
is_administrator "$subject" && exit 77
if [[ "$action" == update ]]; then
  [[ "$confirmation" == "$subject" ]] || exit 64
  [[ -z "$display" ]] || usermod -c "$display" "$subject"
  [[ -z "$shell" ]] || usermod -s "$shell" "$subject"
  # An explicit marker allows clearing the supplementary group list.
  [[ "$groups" == __keep__ ]] || usermod -G "$groups" "$subject"
elif [[ "$action" == delete ]]; then
  [[ "$confirmation" == "$subject" ]] || exit 64
  if pgrep -u "$subject" >/dev/null; then
    printf 'The account still has running processes\n' >&2
    exit 75
  fi
  # Keep the home directory and files for a recoverable administrator review.
  userdel "$subject"
else
  exit 64
fi
