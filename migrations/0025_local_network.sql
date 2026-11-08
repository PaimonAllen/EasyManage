-- Platform direct-egress connectivity, independent from proxy definitions and Agents.
SET LOCAL lock_timeout='2s';
CREATE TABLE IF NOT EXISTS local_network_rounds (
    slot TIMESTAMPTZ PRIMARY KEY,
    checked_at TIMESTAMPTZ NOT NULL,
    results JSONB NOT NULL
);
