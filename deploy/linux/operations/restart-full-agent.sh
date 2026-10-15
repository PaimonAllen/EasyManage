#!/usr/bin/env bash
# Fixed, root-only service restart. No user input or arbitrary command execution.
set -Eeuo pipefail
(( EUID == 0 )) || exit 64
unit=easymanage-agent.service
[[ "$(systemctl show "$unit" --property=LoadState --value)" == loaded ]] || exit 3
[[ "$(systemctl show "$unit" --property=User --value)" == root ]] || exit 3
systemctl is-active --quiet "$unit" || exit 3
systemctl restart "$unit"
sleep 2
systemctl is-active --quiet "$unit"
