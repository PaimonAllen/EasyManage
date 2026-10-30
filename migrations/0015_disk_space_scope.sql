SET LOCAL lock_timeout = '2s';
-- Metadata-only additions; keep all existing scans and their entries.
ALTER TABLE disk_space_scans ADD COLUMN scope TEXT NOT NULL DEFAULT 'directory'
    CHECK (scope IN ('directory', 'all_local'));
-- Only a filesystem root in an all-local snapshot has this path override.
ALTER TABLE disk_space_entries ADD COLUMN scan_path TEXT;
