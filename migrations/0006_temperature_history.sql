-- Nullable additive column: old reports/history remain missing, never zero.
SET LOCAL lock_timeout = '2s';
ALTER TABLE metrics_samples ADD COLUMN IF NOT EXISTS temperatures JSONB;
