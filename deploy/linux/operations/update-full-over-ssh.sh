#!/usr/bin/env bash
# Fixed controller-owned code, never execute the user-writable staged script as root.
set -Eeuo pipefail
umask 077
export PATH=/usr/sbin:/usr/bin:/sbin:/bin LC_ALL=C
[[ $EUID == 0 && $# == 4 ]] || exit 64
stage=$1
expected_node=$2
expected_hash=$3
expected_ca=$4
# Direct deployments already stage the verified candidate in the original
# account's bin directory; SSH-only deployments use a private upload directory.
[[ "$stage" =~ ^(/home/[a-zA-Z0-9_.-]+|/root)/\.easymanage/(staging/agent\.[a-zA-Z0-9]{8}|bin)$ ]] || exit 64
[[ "$expected_hash" =~ ^[a-f0-9]{64}$ && "$expected_ca" =~ ^[a-f0-9]{64}$ ]] || exit 64
[[ "$expected_node" =~ ^[a-f0-9-]{36}$ ]] || exit 64
for path in "$stage/easymanage-agent" /usr/local/sbin/easymanage-agent /etc/easymanage/agent.env /etc/easymanage/tls/ca.crt /var/lib/easymanage-agent; do
    cursor=$path
    while [[ "$cursor" != / ]]; do
        [[ ! -L "$cursor" ]] || exit 64
        cursor=$(dirname -- "$cursor")
    done
done
for path in /usr/local/sbin /usr/local/sbin/easymanage-agent /etc/easymanage /etc/easymanage/tls /etc/easymanage/agent.env /etc/easymanage/tls/ca.crt /var/lib/easymanage-agent; do
    [[ "$(stat -c %u "$path")" == 0 ]] || exit 64
    mode=$(stat -c %a "$path")
    (( (8#$mode & 0022) == 0 )) || exit 64
done
[[ "$(systemctl show easymanage-agent.service -p LoadState --value)" == loaded ]] || exit 3
service_user="$(systemctl show easymanage-agent.service -p User --value)"
[[ "$service_user" == root || -z "$service_user" ]] || exit 3
[[ "$(sed -n 's/^EASYMANAGE_NODE_ID=//p' /etc/easymanage/agent.env)" == "$expected_node" ]] || exit 64
printf '%s  %s\n' "$expected_ca" /etc/easymanage/tls/ca.crt | sha256sum --check --status
[[ -f "$stage/easymanage-agent" && "$(stat -c %s "$stage/easymanage-agent")" -le 268435456 ]] || exit 64
backup=$(mktemp -d /var/lib/easymanage-agent/update-backup.XXXXXXXX)
# Copy into a root-only directory BEFORE validating; user cannot race the source
# after validation to substitute code run by the Full service.
install -m 0700 "$stage/easymanage-agent" "$backup/candidate"
printf '%s  %s\n' "$expected_hash" "$backup/candidate" | sha256sum --check --status
cp -p -- /usr/local/sbin/easymanage-agent "$backup/previous-agent"
changed=false
complete=false
recover() {
    result=$?
    trap - EXIT
    if [[ "$changed" == true && "$complete" != true ]]; then
        set +e
        restore=$(mktemp /usr/local/sbin/.easymanage-restore.XXXXXXXX)
        install -m 0755 "$backup/previous-agent" "$restore"
        mv -T -- "$restore" /usr/local/sbin/easymanage-agent
        timeout 15s systemctl restart easymanage-agent.service
        [[ $result != 0 ]] || result=1
    fi
    exit "$result"
}
trap recover EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
temporary=$(mktemp /usr/local/sbin/.easymanage-agent.XXXXXXXX)
install -m 0755 "$backup/candidate" "$temporary"
mv -T -- "$temporary" /usr/local/sbin/easymanage-agent
changed=true
timeout 15s systemctl restart easymanage-agent.service
sleep 2
systemctl is-active --quiet easymanage-agent.service
complete=true
printf 'Full binary updated; SQLite identity and system configuration retained. Await fresh heartbeat.\n'
