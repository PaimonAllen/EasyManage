-- Stop the new platform first. Restore the previous single-profile policy only.
-- Imported ciphertext, names, filenames, scores and observations are retained.
BEGIN;
SET LOCAL lock_timeout='2s';
UPDATE proxy_lab_profiles SET enabled=FALSE
WHERE enabled AND id<>(SELECT id FROM proxy_lab_profiles WHERE enabled ORDER BY created_at,id LIMIT 1);
CREATE UNIQUE INDEX IF NOT EXISTS proxy_lab_one_enabled ON proxy_lab_profiles(enabled) WHERE enabled;
ALTER TABLE proxy_lab_profiles ALTER COLUMN enabled SET DEFAULT FALSE;
DELETE FROM _sqlx_migrations WHERE version=24;
COMMIT;
