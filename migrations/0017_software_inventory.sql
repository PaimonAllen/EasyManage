-- Stable package lists must not be rewritten in every dynamic heartbeat.
SET LOCAL lock_timeout = '2s';
-- The one-second live view must not decompress a full snapshot to read control.
ALTER TABLE metrics_latest ADD COLUMN agent_control JSONB;
UPDATE metrics_latest SET agent_control=metrics->'agent_control';
CREATE TABLE node_software_inventory (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    collected_at TIMESTAMPTZ NOT NULL,
    packages JSONB NOT NULL CHECK (jsonb_typeof(packages) = 'array')
);
INSERT INTO node_software_inventory(node_id,collected_at,packages)
SELECT node_id,collected_at,metrics->'software_packages'
FROM metrics_latest WHERE jsonb_typeof(metrics->'software_packages')='array';
UPDATE metrics_latest SET metrics=metrics-'software_packages'
WHERE metrics ? 'software_packages';
