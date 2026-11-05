-- Stop the platform before using this compatibility rollback. State is retained.
BEGIN;
SET LOCAL lock_timeout='2s';
DELETE FROM _sqlx_migrations WHERE version=21;
COMMIT;
