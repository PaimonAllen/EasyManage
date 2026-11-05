-- Schema 20 -> 19 compatibility rollback, ONLY with the platform stopped.
-- Retain external links and their soft-delete state for a subsequent upgrade.
BEGIN;
SET LOCAL lock_timeout = '2s';
DELETE FROM _sqlx_migrations WHERE version = 20;
COMMIT;
