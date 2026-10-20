-- Independent per-node user/session collection switch; absent rows mean enabled.
SET LOCAL lock_timeout = '2s';
CREATE TABLE node_user_collection (
    node_id UUID PRIMARY KEY REFERENCES nodes(id) ON DELETE CASCADE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
