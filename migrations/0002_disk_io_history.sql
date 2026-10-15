-- Nullable columns keep pre-upgrade samples and older Agents readable.
-- Adding nullable columns without defaults avoids rewriting the historical table.
SET LOCAL lock_timeout = '2s';

ALTER TABLE metrics_samples
    ADD COLUMN IF NOT EXISTS disk_collected_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS disk_read_bytes_per_second DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS disk_written_bytes_per_second DOUBLE PRECISION;
