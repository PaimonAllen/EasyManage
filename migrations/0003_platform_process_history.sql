-- Nullable Agent columns preserve all existing whole-host samples without rewriting them.
-- Historical process metrics only exist from the first report after this migration.
SET LOCAL lock_timeout = '2s';

ALTER TABLE metrics_samples
    ADD COLUMN IF NOT EXISTS agent_pid BIGINT,
    ADD COLUMN IF NOT EXISTS agent_cpu_percent_of_host DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS agent_resident_memory_bytes BIGINT,
    ADD COLUMN IF NOT EXISTS agent_disk_read_bytes_per_second DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS agent_disk_written_bytes_per_second DOUBLE PRECISION;

CREATE TABLE IF NOT EXISTS platform_process_samples (
    id BIGSERIAL PRIMARY KEY,
    sampled_at TIMESTAMPTZ NOT NULL,
    pid BIGINT NOT NULL CHECK (pid > 0),
    cpu_percent_of_host DOUBLE PRECISION,
    resident_memory_bytes BIGINT,
    disk_read_bytes_per_second DOUBLE PRECISION,
    disk_written_bytes_per_second DOUBLE PRECISION
);

CREATE INDEX IF NOT EXISTS platform_process_samples_time_idx
    ON platform_process_samples(sampled_at DESC);
