-- Process lifecycle is independent of cooperative monitoring pause.
SET LOCAL lock_timeout = '2s';
CREATE TABLE IF NOT EXISTS agent_service_controls (
    node_id UUID PRIMARY KEY REFERENCES nodes(id),
    desired_stopped BOOLEAN NOT NULL DEFAULT FALSE,
    state TEXT NOT NULL CHECK(state IN ('stopping','stopped','redeploying','running','unknown')),
    revision BIGINT NOT NULL CHECK(revision BETWEEN 1 AND 9007199254740991),
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
