-- Stop the platform first. Old binaries cannot enforce the new master switch.
-- Preserve per-source controls and all observations for a later upgrade.
BEGIN;
SET LOCAL lock_timeout='2s';
DELETE FROM _sqlx_migrations WHERE version=26;
COMMIT;
