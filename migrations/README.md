# Database migrations

`0016_disk_space_directory_summaries.sql` is a destructive **scan-metadata-only**
conversion. Back up before upgrading and stop the previous platform process.
It retains usable directory trees and scan task history, converts legacy JSON,
and removes historical file-level details plus unusable unfinished entry sets.
The old heap and four indexes are dropped after copying directory summaries to
`disk_space_directories` (primary key + parent index). This physically releases
scan storage without a full-database VACUUM FULL. Node identities, credentials,
metrics and Agent databases are untouched. Restoring file-level history requires
the pre-upgrade backup; an older backend cannot use this schema.


`0015_disk_space_scope.sql` adds an explicit directory/all-local scan scope and
nullable filesystem-root paths. Existing snapshots keep directory scope. These
metadata additions use a 2-second lock timeout; no entry contents are rewritten.

`0014_disk_space_streaming.sql` adds per-entry disk scan storage and directory
ordering indexes. Unlimited scans stream bounded batches instead of building one
large JSON value. Legacy snapshots are unchanged; new entry rows cascade when
their retained scan is removed. This additive migration uses a 2-second lock
timeout and does not rewrite existing metrics or scan results.

`0013_disk_space_scans.sql` adds a separate bounded history of on-demand read-only
disk scans. It creates a new table and two indexes, including one active scan per
node, with a 2-second lock timeout. Existing metrics, identities and credentials
are not rewritten. Startup marks unfinished scans interrupted; retention keeps
three snapshots per node/permission level, no more than seven days and thirty
completed snapshots globally. Platform file rollback leaves this additive table
in place. No filesystem scan is triggered by the migration or deployment.

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

`0009_node_user_collection.sql` creates a per-node user/session collection switch.
Absent rows default to enabled. It does not rewrite metrics, change accounts or
services, or remove history. The foreign key uses a 2-second lock timeout; apply
only during an approved platform update. Update the platform before Agents.

`0010_node_permission_collection.sql` adds a separate per-node permission-audit
switch, defaulting to enabled. It never changes filesystem/account permissions or
rewrites history. It uses a 2-second lock timeout; deploy the platform first, then
Agents, only in an approved production update window.

`0011_node_monitoring_pause.sql` stores per-node monitoring intent, revision and
Agent acknowledgement/control timestamp. Defaults retain current monitoring.
It does not delete history, rotate credentials, stop services or change privileges.
Adding columns requires a brief exclusive table lock, bounded by a 2-second lock
timeout. Apply only during an approved platform update, then update Agents.

`0012_node_proxy_collection.sql` adds an independent per-node proxy-monitoring
switch, default enabled. It never stops real proxies or modifies their settings,
accounts or history. The foreign key takes a brief table lock, bounded by a
2-second lock timeout; apply only during an approved platform update, before
updating Agents. Older Agent report/control peers omit unsupported policy flags.
