-- Per-machine proxy monitoring only; never stop/reconfigure the actual proxy.
SET LOCAL lock_timeout = '2s';
CREATE TABLE node_proxy_collection (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
