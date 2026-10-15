#!/usr/bin/env bash
# Deployment assets are versioned here; local configuration stays outside Git.
# Read-only storage inventory. No sudo, disk scans, SMART self-tests or mutations.
set -u
export LC_ALL=C

printf 'Host: %s\nUser: %s\n' "$(hostname)" "$(id -un)"
printf '\nPhysical disks and partitions (sizes in bytes):\n'
timeout 15s lsblk --json --bytes --exclude 7 --output NAME,PATH,TYPE,SIZE,ROTA,TRAN,MODEL,FSTYPE,FSVER,FSAVAIL,FSUSE%,MOUNTPOINTS,PTTYPE || true

printf '\nLocal filesystem capacity (bytes; non-local filesystems excluded):\n'
timeout 15s df --local -B1 --output=source,fstype,size,used,avail,pcent,target || true

printf '\nMounted disk filesystems:\n'
timeout 10s findmnt --json --real --output SOURCE,TARGET,FSTYPE --types ext4,xfs,btrfs,vfat,ntfs,ntfs3,fuseblk,zfs || true

printf '\nActive swap (bytes):\n'
timeout 5s swapon --show --bytes --output NAME,TYPE,SIZE,USED,PRIO || true

printf '\nPersistent disk mounts (credentials and non-device sources omitted):\n'
if [[ -r /etc/fstab ]]; then
    awk '
        /^[[:space:]]*#/ || NF < 4 { next }
        $1 !~ /^(UUID=|PARTUUID=|LABEL=|PARTLABEL=|\/dev\/)/ { next }
        {
            count = split($4, options, ","); safe = ""
            for (i = 1; i <= count; i++) {
                option = options[i]
                if (tolower(option) ~ /(pass(word)?|credentials?|secret|token|username|user)=/) option = "[redacted]"
                safe = safe (safe == "" ? "" : ",") option
            }
            print $1, $2, $3, safe, $5, $6
        }
    ' /etc/fstab
fi

printf '\nSoftware storage layers (names only, no privileged probes):\n'
if [[ -r /proc/mdstat ]]; then timeout 5s sed -n '1,60p' /proc/mdstat; fi
if [[ -d /sys/class/block ]]; then
    for task_device in /sys/class/block/dm-*; do
        [[ -r "$task_device/dm/name" ]] || continue
        printf 'Device mapper: '
        timeout 5s head -c 256 -- "$task_device/dm/name"
        printf '\n'
    done
fi
