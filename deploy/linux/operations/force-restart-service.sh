#!/usr/bin/env bash
# Fixed services only; called after verified administrator SSH/sudo authentication.
set -Eeuo pipefail
(( EUID == 0 )) || exit 64
action=${1:-}
owner=${2:-}
[[ "$action" == agent || "$action" == platform ]] || exit 64
[[ "$owner" =~ ^[a-zA-Z_][a-zA-Z0-9_.\$-]{0,63}$ && "$owner" != root ]] || exit 64
uid=$(id -u "$owner")
(( uid >= 1000 )) || exit 64
user_ctl() {
    /usr/sbin/runuser -u "$owner" -- /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin \
        XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" \
        /usr/bin/systemctl --user "$@"
}
force_restart() {
    local unit=$1
    shift
    "$@" stop --no-block "$unit"
    "$@" kill --kill-who=all --signal=SIGKILL "$unit" 2>/dev/null || true
    "$@" reset-failed "$unit"
    "$@" restart "$unit"
    sleep 2
    "$@" is-active --quiet "$unit"
}
if [[ "$action" == platform ]]; then
    unit=easymanage-server.service
    [[ "$(user_ctl show "$unit" -p LoadState --value)" == loaded ]] || exit 3
    expected="/home/$owner/.local/share/easymanage/bin/easymanage-server"
    exec_start=$(user_ctl show "$unit" -p ExecStart --value)
    [[ "$exec_start" == *"path=$expected ;"* ]] || exit 3
    # Run from the system manager, outside the platform's own cgroup/SSH request.
    # shellcheck disable=SC2016
    /usr/bin/systemd-run --quiet --collect --on-active=3s \
        --timer-property=AccuracySec=1s --property=TimeoutStartSec=45s \
        /usr/sbin/runuser -u "$owner" -- /usr/bin/env -i PATH=/usr/sbin:/usr/bin:/sbin:/bin \
        XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" \
        /bin/bash --noprofile --norc -c '
            set -Eeuo pipefail
            unit=easymanage-server.service
            systemctl --user stop --no-block "$unit"
            systemctl --user kill --kill-who=all --signal=SIGKILL "$unit" 2>/dev/null || true
            systemctl --user reset-failed "$unit"
            systemctl --user restart "$unit"
            sleep 2
            systemctl --user is-active --quiet "$unit"
        '
    printf 'Platform force restart scheduled.\n'
else
    unit=easymanage-agent.service
    state=$(systemctl show "$unit" -p ActiveState --value)
    if [[ "$(systemctl show "$unit" -p LoadState --value)" == loaded ]] && \
       { [[ "$state" != inactive ]] || systemctl is-enabled --quiet "$unit"; }; then
        [[ "$(systemctl show "$unit" -p User --value)" == root ]] || exit 3
        force_restart "$unit" /usr/bin/systemctl
        printf 'Full Agent force restarted; root retained.\n'
    else
        [[ "$(user_ctl show "$unit" -p LoadState --value)" == loaded ]] || exit 3
        user_state=$(user_ctl show "$unit" -p ActiveState --value)
        [[ "$user_state" != inactive ]] || user_ctl is-enabled --quiet "$unit" || exit 3
        # Never silently start Basic while an enabled/failed Full unit exists.
        force_restart "$unit" user_ctl
        printf 'Basic Agent force restarted.\n'
    fi
fi
