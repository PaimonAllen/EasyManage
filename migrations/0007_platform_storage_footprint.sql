-- Sparse, long-interval EasyManage-only storage samples. Existing data is untouched.
SET LOCAL lock_timeout = '2s';

CREATE TABLE IF NOT EXISTS server_storage_samples (
    sampled_at TIMESTAMPTZ PRIMARY KEY,
    binary_bytes BIGINT CHECK (binary_bytes >= 0),
    web_bytes BIGINT CHECK (web_bytes >= 0),
    database_bytes BIGINT CHECK (database_bytes >= 0),
    total_bytes BIGINT CHECK (total_bytes >= 0)
);

CREATE TABLE IF NOT EXISTS agent_storage_samples (
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    sampled_at TIMESTAMPTZ NOT NULL,
    binary_bytes BIGINT CHECK (binary_bytes >= 0),
    state_bytes BIGINT CHECK (state_bytes >= 0),
    total_bytes BIGINT CHECK (total_bytes >= 0),
    PRIMARY KEY (node_id, sampled_at)
);

CREATE INDEX IF NOT EXISTS agent_storage_samples_time_idx
    ON agent_storage_samples(sampled_at DESC);
