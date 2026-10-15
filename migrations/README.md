# Database migrations

Versioned PostgreSQL migrations for the controller belong here.

Migrations are embedded in `easymanage-server` and applied automatically at startup. Never
edit an applied migration; add a new numbered migration instead.

`0002_disk_io_history.sql` adds three nullable metadata-only columns for the
overview disk I/O chart. It does not rewrite old samples. The migration sets a
2-second lock timeout; a busy production table causes startup to fail safely
rather than wait indefinitely. Apply it only during an approved maintenance
window, before rolling out the matching Server binary.

`0003_platform_process_history.sql` adds nullable Agent process columns to the
existing sample table and creates a separate, indexed controller-process sample
table. Existing rows are not rewritten and have unknown process metrics; they
must not be treated as zero. The `ALTER TABLE` needs a brief exclusive lock and
uses the same 2-second lock timeout. Roll out the matching Server binary only in
an approved production maintenance window; the migration runs at startup.

`0007_platform_storage_footprint.sql` creates separate, sparse 10-minute storage
history tables for the Server and Agents. It never rewrites existing metrics or
scans host disks. The Server migration runs at startup; test and schedule the
restart before production rollout. Historical values start at first collection.

`0008_machine_management.sql` adds display labels, notes and soft-delete timestamps
without removing node/history/credential rows. A separate table stores hashed,
expiring, single-node onboarding tokens, consumed within the enrollment transaction.
Deletion/restore/enrollment share a per-node advisory transaction lock. The schema
change needs a brief exclusive lock with a 2-second lock timeout; deploy only in
an approved maintenance window. No remote service is changed by this migration.
