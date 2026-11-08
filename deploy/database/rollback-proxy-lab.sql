-- Stop the new platform before rollback. Preserve imported ciphertext and observations.
-- For schema 25+, first run rollback-local-network.sql.
-- Then for schema 24+, run rollback-proxy-lab-queue.sql.
-- The previous platform ignores these independent tables; no monitoring data is deleted.
BEGIN;
SET LOCAL lock_timeout='2s';
DELETE FROM _sqlx_migrations WHERE version=23;
COMMIT;
