-- Emergency schema-18 -> schema-16 rollback, ONLY with the platform stopped.
-- Existing monitoring history is retained. Newly added precision fields are
-- removed; software is restored to the legacy latest-snapshot representation.
BEGIN;
SET LOCAL lock_timeout='2s';
UPDATE metrics_latest m SET metrics=m.metrics || jsonb_build_object('software_packages',p.packages)
FROM node_software_inventory p WHERE p.node_id=m.node_id;
ALTER TABLE metrics_latest DROP COLUMN IF EXISTS agent_control;
ALTER TABLE platform_process_samples
    DROP COLUMN IF EXISTS sample_interval_seconds,
    DROP COLUMN IF EXISTS cpu_percent_of_one_core,
    DROP COLUMN IF EXISTS service_cpu_percent_of_one_core,
    DROP COLUMN IF EXISTS service_memory_current_bytes;
ALTER TABLE metrics_samples
    DROP COLUMN IF EXISTS agent_sample_interval_seconds,
    DROP COLUMN IF EXISTS agent_cpu_percent_of_one_core,
    DROP COLUMN IF EXISTS agent_service_cpu_percent_of_one_core,
    DROP COLUMN IF EXISTS agent_service_memory_current_bytes;
DROP TABLE node_software_inventory;
DELETE FROM _sqlx_migrations WHERE version IN (17,18);
COMMIT;
