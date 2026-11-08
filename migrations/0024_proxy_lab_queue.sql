-- All imported profiles join the controller's single sequential availability queue.
SET LOCAL lock_timeout='2s';
DROP INDEX IF EXISTS proxy_lab_one_enabled;
ALTER TABLE proxy_lab_profiles ALTER COLUMN enabled SET DEFAULT TRUE;
ALTER TABLE proxy_lab_profiles ADD COLUMN IF NOT EXISTS original_filename TEXT;
UPDATE proxy_lab_profiles
SET original_filename=substr(source,char_length('本机 · ')+1)
WHERE original_filename IS NULL AND source LIKE '本机 · %';
-- The old exclusive switch disabled other imports implicitly. Enqueue them all.
UPDATE proxy_lab_profiles SET enabled=TRUE,updated_at=NOW() WHERE NOT enabled;
