# Windows Agent (PowerShell 7)

The controller remains on Linux. Windows x64 is a native Agent target; macOS is not implemented. Windows operations explicitly launch PowerShell 7 (`pwsh.exe`); they do not change the OpenSSH default shell, firewall, UAC policy or execution policy. PowerShell 5.1 is not a runtime substitute.

## Prerequisites

- Windows 10/11 or Windows Server compatible with the selected Rust toolchain and PowerShell 7 with OpenSSH Server and its SFTP subsystem enabled.
- PowerShell 7+ installed and accessible to the execution account. SSH management and the native Agent share discovery: try the configured hint, PATH, common MSI directories, registered install locations, and the current account WindowsApps package. Each candidate is executed to verify version 7+ and resolve the real executable. Stale recorded paths fall back to discovery. Built-in Windows PowerShell is used only for discovery; workloads run in pwsh. No PATH or SSH default-shell changes are made. Per-user MSIX availability does not imply LocalSystem access. Full preflight requires a machine-wide non-MSIX executable whose file/ancestor owners and write ACLs are limited to SYSTEM, Administrators or TrustedInstaller; reparse paths are refused. It selects a machine installation even when SSH resolves to a user package. If none is found, onboarding stops before creating managed directories or registering a node. An MSI or administrator-protected ZIP installation works; there is no hard-coded drive or install path. Native SCM processes retain the same discovery restriction on future restarts.
- For Full installation, the SSH login must have an actual elevated Administrator token. Group membership alone is insufficient; EasyManage does not bypass UAC.
- The controller must publish a matching Windows PE candidate. Linux ELF candidates are never sent to Windows.

## Add Machine

Choose Windows in Add Machine, confirm the SSH host-key fingerprint, choose Full or Basic and supply SSH authentication. Binary/configuration files are sent over SFTP into a private staging directory; SHA-256 verification happens before installation. Scoped enrollment tokens are sent only over SSH stdin/private files. The CA certificate is preserved and TLS/hostname verification is not disabled.

Full uses the native `EasyManageAgent` Windows service under LocalSystem, with startup/recovery configured in SCM. Files live under `%ProgramData%/EasyManage/{bin,config,state,logs}`. Credentials and SQLite files inherit an ACL limited to SYSTEM and Administrators. The `--service --config <absolute-path>` entrypoint handles SCM stop/shutdown, allowing pending SQLite operations to close before reporting Stopped. Operational logs are in `logs/agent.log`, with an 8 MiB startup rotation limit and one previous file.

Basic uses a Limited/Interactive scheduled task named `EasyManageAgent-<SID>`, installed under `%LOCALAPPDATA%/EasyManage`. It runs only while that user is logged on and does not promise unattended boot/logoff operation. Choose Full for an always-on monitoring node. Basic-to-Full elevation explicitly verifies an administrator, disables the old task, copies the stopped SQLite database and WAL, preserves UUID/token/outbox and starts a native Full service. The original files remain for recovery.

Automatic connection mode uses a controller-initiated, loopback-only SSH reverse HTTPS transport on Windows. Direct mode requires the Agent to reach the controller HTTPS URL. No public listening port is added. Persistent transport reconnect verifies the original host key and reuses the original identity.

## Observations

Fast CPU, memory and interface counters use native sysinfo backends. CPU/GPU sampling is independent of slow inventories. GPU readings use optional NVIDIA tools; missing sensors are unavailable, never fabricated zero values. Windows has no POSIX load average; these fields are null. UID/GID fields are legacy compatibility fields (`u32::MAX` on Windows); use native SIDs and the reported Administrator token.

PowerShell inventory reads CIM/registry/service/network/account/NTFS metadata only. Optional Tailscale/ZeroTier status uses shared bounded CLI probes and never changes network membership. It never queries `Win32_Product` (which can trigger MSI repair). User, proxy/AI, software/services, programming and hardware observations retain independent timestamps. Failed or denied categories preserve cached timestamps and report issues. Current intervals are 3 minutes for users, 5 minutes for proxies/AI, 1 hour for software and disk topology, 12 hours for programming tools and 24 hours for hardware; configurable network/permission intervals follow collection policy.

Directory analysis streams directory summaries only. It uses handle-based native allocation/identity metadata, deduplicates hard links, supports cancellation/time/entry budgets and skips reparse points. UNC/network drives are not scanned. Windows local account management protects built-in/current/admin/active users; newly created accounts are disabled until authentication is configured separately. Deletion retains profile files.

## Build and verification

On the Linux build host, install the Rust `x86_64-pc-windows-gnu` target and provide an x64 MinGW compiler. `scripts/build-windows-agent.sh --check` checks Windows code; `--release` builds `target/x86_64-pc-windows-gnu/release/easymanage-agent.exe`; `--release --publish` stages a candidate without changing running services. An extracted compiler may be kept under ignored `third_party/windows-build/root`, avoiding production package installation.

`easymanage-agent.exe --diagnostics <new-test-directory>` performs bounded read-only observations, creates only a fresh diagnostic SQLite file and does not load production configuration or contact a controller. Never point diagnostics at an existing Agent state directory. Service installation/restart, live updates, account mutation, privilege migration and full-disk/load tests are separate operations; on an in-use test host they require an explicit maintenance scope. Use isolated fixtures for scanner verification and a disposable Windows VM for destructive/lifecycle fault tests.

## Read-only Windows verification (2026-11-12)

PowerShell 7.7.0 installed as a WindowsApps package was reachable as bare `pwsh` from a fresh SSH session under the existing login account. The actual package executable path was resolved without changing PATH, SSH defaults or execution policy. This proves availability for that SSH account, not a LocalSystem/package-registration claim.

The compiled x64 Agent diagnostics collected native CPU/memory/GPU/disk/network, users, software/services, hardware and permissions without collection issues. The actual SSH encoded bootstrap and read-only native account listing were exercised. All deployment scripts passed the native PowerShell parser. The guarded `tests/integration/windows_readonly_scan.ps1` uses only a fresh `%TEMP%/EasyManageReadonly-<UUID>` tree to verify metadata-only output, Unicode directory names, hard-link deduplication, junction exclusion/root refusal, entry limits, cancellation and actual privilege checks. NTFS directory-index metadata is included in directory sizes.

The scanner's `skipped_mounts` field now has an explicit initializer because the actual pwsh/Add-Type pipeline rejected its unassigned-field warning. SSH-config hash, firewall profiles, local-account inventory and EasyManage service/task baseline remained unchanged. No Agent service/task, account mutation, elevation, update or business-directory scan was executed. SCM lifecycle and LocalSystem access to the chosen PowerShell package remain outside this read-only verification.

Automatic discovery also passed native read-only regression with a missing pwsh PATH, a stale explicit path, and current-user install-record fallback (5 checks). The native Agent completed diagnostics with a stripped process PATH and stale override, with no collection issues. SSH launch preserves the original stdin stream for large scripts, uses UTF-8 output, and propagates the selected shell exit status. No machine PATH, service, account or firewall settings were changed. Linux management continues to use Bash by default.

Install preparation guards also refuse a nonempty new managed directory before any ACL mutation, recheck for services/tasks that appeared after staging, and verify both scheduled-task executable and config arguments before stopping/starting/promoting it. `windows_install_guards.ps1` exercises synthetic ACL and task objects plus one guarded temporary nonempty directory; it never registers/stops a real task or service.

Generated dotenv paths are single-quoted, including `Program Files` and Basic profiles containing spaces or Unicode. The production environment renderer is parsed by the same dotenv library in regression tests. Identity checks accept new quoted and previous unquoted values while refusing duplicate identity keys. Basic-to-Full migration rewrites the state/CA/pwsh paths in the same quoted format before starting SCM.

Full managed directories are owned by Administrators. Their inheritable ACL includes an OWNER RIGHTS read-permissions ACE so a file created by the SSH account does not regain implicit WRITE_DAC through ownership when its token is not elevated; SYSTEM and Administrators retain FullControl. Basic retains its current-user ownership. The ACL descriptor regression builds these security descriptors in memory with all filesystem mutators mocked. See [Microsoft OWNER RIGHTS semantics](https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/manage/understand-security-identifiers).

## Full onboarding verification (2026-11-13)

After installing machine-wide PowerShell 7.6.6, the native discovery/ACL preflight selected the protected Program Files executable while the SSH login could still resolve the user MSIX package. The first Full onboarding completed through the platform: SFTP/hash checks, SCM startup and a fresh authenticated heartbeat all succeeded. A subsequent read-only check confirmed an online Windows node reporting Full with native principal `S-1-5-18`, and an Automatic/Running service under LocalSystem. This validates first installation and reporting; it does not claim fault injection, account mutation, live update or Basic-to-Full migration was exercised on the in-use host.
