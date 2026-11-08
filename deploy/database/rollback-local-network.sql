-- Stop the new platform first. Preserve the independent local network observations.
-- Run before starting a platform binary compiled without schema 25.
BEGIN;
SET LOCAL lock_timeout='2s';
DELETE FROM _sqlx_migrations WHERE version=25;
COMMIT;
