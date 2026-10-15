# Fleet orchestration

`fleet.py` coordinates trusted inventories, verified SSH targets, candidate publication and distribution. It calls OS-specific assets rather than owning service units or native installers. Current commands and targets are Linux-only; moving this file does not make it a cross-platform installer.

The usual entry point remains `scripts/run-server.sh`. Private inventory stays at ignored `deploy/local/fleet.json`; the public template is `deploy/examples/fleet.example.json` and cannot be executed as-is. Existing inventories without `platform` retain Linux behavior; explicitly unsupported platforms are refused before SSH/service operations.

New release packages use this layout. The Server retains a validated fallback for already-published packages containing `deploy/linux/fleet.py`. Bundle payload basenames and checksums remain unchanged; installed paths and node identity are not migrated.

Before adding Windows/macOS targets, implement platform detection, OS-specific artifact selection, native service operations, credential checks and rollback tests. Never send Linux Bash/sudo/systemd scripts to another OS. A user-mode Server may manage Full Agents.
