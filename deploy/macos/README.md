# macOS deployment preparation

Status: planned only. There is no macOS Agent backend, installer or validated deployment command yet. Do not run Linux deployment scripts on macOS.

Implement and test before enabling this target:

1. Native collection and explicit capability/privilege reporting; do not reuse Linux `/proc`, `/sys`, ACL or device rules.
2. Native configuration/state paths and protected credentials/SQLite identity for normal users and privileged services.
3. launchd service assets and lifecycle management, with user agents and system daemons kept distinct.
4. OS/architecture-specific artifacts, authenticated update/restart, checksum validation, rollback and persistent node identity; verify macOS signing/distribution requirements.
5. TLS trust, offline reporting, missing sensors, normal/admin execution and restart/update integration tests on macOS.

Add real installers and launchd assets here once implemented, not empty subdirectories or no-op scripts. Shared Agent logic stays in `agent/src/`; platform-specific collection belongs in `agent/src/platform/macos/` when it exists. See [architecture](../../docs/architecture.md).
