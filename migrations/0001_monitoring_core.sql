CREATE TABLE IF NOT EXISTS nodes (
    id UUID PRIMARY KEY,
    hostname TEXT NOT NULL,
    os_name TEXT NOT NULL,
    os_version TEXT NOT NULL,
    kernel_version TEXT NOT NULL,
    architecture TEXT NOT NULL,
    cpu_model TEXT NOT NULL,
    cpu_logical_cores INTEGER NOT NULL CHECK (cpu_logical_cores >= 0),
    cpu_physical_cores INTEGER CHECK (cpu_physical_cores >= 0),
    agent_version TEXT NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS agents (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    token_hash CHAR(64) NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_authenticated_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS agents_token_hash_key ON agents(token_hash);

CREATE TABLE IF NOT EXISTS metrics_latest (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    collected_at TIMESTAMPTZ NOT NULL,
    metrics JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS metrics_samples (
    id BIGSERIAL PRIMARY KEY,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    collected_at TIMESTAMPTZ NOT NULL,
    cpu_usage_percent DOUBLE PRECISION NOT NULL,
    memory_used_bytes BIGINT NOT NULL CHECK (memory_used_bytes >= 0),
    network_received_bytes_per_second DOUBLE PRECISION NOT NULL,
    network_transmitted_bytes_per_second DOUBLE PRECISION NOT NULL,
    gpu_metrics JSONB NOT NULL
);

CREATE INDEX IF NOT EXISTS metrics_samples_node_time_idx
    ON metrics_samples(node_id, collected_at DESC);

CREATE TABLE IF NOT EXISTS audit_logs (
    id BIGSERIAL PRIMARY KEY,
    actor_type TEXT NOT NULL,
    actor_id TEXT,
    action TEXT NOT NULL,
    target_type TEXT NOT NULL,
    target_id TEXT,
    details JSONB NOT NULL DEFAULT '{}'::JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

