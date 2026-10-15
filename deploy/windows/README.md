# Windows deployment preparation

Status: planned only. There is no Windows Agent backend, installer or validated deployment command yet. Do not run Linux deployment scripts on Windows.

Implement and test before enabling this target:

1. A native Agent collector and compatible capability/administrator reporting; Windows administrator status is not Linux UID 0.
2. Native configuration/state paths and restrictive file ACLs for credentials and SQLite identity.
3. Windows Service installation and lifecycle management; keep user-mode execution distinct from service privileges.
4. OS/architecture-specific artifacts, authenticated update/restart, checksum validation, rollback and persistent node identity.
5. TLS trust, offline reporting, missing metrics, normal/admin execution and restart/update integration tests on Windows.

Add real installers and service assets here once implemented, not empty subdirectories or no-op scripts. Shared Agent logic stays in `agent/src/`; platform-specific collection belongs in `agent/src/platform/windows/` when it exists. See [architecture](../../docs/architecture.md).
