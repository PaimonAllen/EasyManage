-- Nullable fields: old Agents and historical samples remain explicitly unknown.
SET LOCAL lock_timeout = '2s';
ALTER TABLE platform_process_samples
    ADD COLUMN sample_interval_seconds DOUBLE PRECISION,
    ADD COLUMN cpu_percent_of_one_core DOUBLE PRECISION,
    ADD COLUMN service_cpu_percent_of_one_core DOUBLE PRECISION,
    ADD COLUMN service_memory_current_bytes BIGINT;
ALTER TABLE metrics_samples
    ADD COLUMN agent_sample_interval_seconds DOUBLE PRECISION,
    ADD COLUMN agent_cpu_percent_of_one_core DOUBLE PRECISION,
    ADD COLUMN agent_service_cpu_percent_of_one_core DOUBLE PRECISION,
    ADD COLUMN agent_service_memory_current_bytes BIGINT;
