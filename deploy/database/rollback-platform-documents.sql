-- Schema 19 -> 18 compatibility rollback, ONLY with the platform stopped.
-- Keep platform_documents and all saved Markdown. Migration 19 uses IF NOT
-- EXISTS so a later upgrade retains edits made before this rollback.
BEGIN;
SET LOCAL lock_timeout = '2s';
DELETE FROM _sqlx_migrations WHERE version = 19;
COMMIT;
