-- Independent, default-enabled permission collection switch; no history rewrite.
SET LOCAL lock_timeout = '2s';
CREATE TABLE node_permission_collection (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
