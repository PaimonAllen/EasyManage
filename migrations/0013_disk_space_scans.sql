SET LOCAL lock_timeout = '2s';
CREATE TABLE disk_space_scans (
    id UUID PRIMARY KEY,
    node_id UUID NOT NULL REFERENCES nodes(id),
    mode TEXT NOT NULL CHECK (mode IN ('basic', 'full')),
    root_path TEXT NOT NULL CHECK (octet_length(root_path) <= 2048),
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'partial', 'cancelled', 'failed', 'interrupted')),
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    finished_at TIMESTAMPTZ,
    progress JSONB NOT NULL DEFAULT '{}'::jsonb,
    result JSONB,
    message TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX disk_space_one_active_per_node ON disk_space_scans(node_id) WHERE status = 'running';
CREATE INDEX disk_space_recent ON disk_space_scans(node_id, started_at DESC);
