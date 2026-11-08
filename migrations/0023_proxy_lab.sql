-- Platform-only proxy configuration, manual reputation checks and 3-minute observations.
SET LOCAL lock_timeout='2s';
CREATE TABLE IF NOT EXISTS proxy_lab_profiles (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    revision BIGINT NOT NULL DEFAULT 1,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    warnings JSONB NOT NULL DEFAULT '[]',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE UNIQUE INDEX IF NOT EXISTS proxy_lab_one_enabled ON proxy_lab_profiles(enabled) WHERE enabled;
CREATE TABLE IF NOT EXISTS proxy_lab_nodes (
    id UUID PRIMARY KEY,
    profile_id UUID NOT NULL REFERENCES proxy_lab_profiles(id) ON DELETE CASCADE,
    identity_hash TEXT NOT NULL,
    name TEXT NOT NULL,
    protocol TEXT NOT NULL,
    region TEXT NOT NULL,
    unsupported TEXT,
    definition BYTEA NOT NULL,
    revision BIGINT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT TRUE,
    UNIQUE(profile_id,identity_hash)
);
CREATE INDEX IF NOT EXISTS proxy_lab_nodes_profile ON proxy_lab_nodes(profile_id) WHERE active;
CREATE TABLE IF NOT EXISTS proxy_lab_rounds (
    profile_id UUID NOT NULL REFERENCES proxy_lab_profiles(id) ON DELETE CASCADE,
    slot TIMESTAMPTZ NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL,
    revision BIGINT NOT NULL,
    results JSONB NOT NULL,
    PRIMARY KEY(profile_id,slot)
);
CREATE TABLE IF NOT EXISTS proxy_lab_scores (
    node_id UUID NOT NULL REFERENCES proxy_lab_nodes(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    revision BIGINT NOT NULL,
    ip INET,
    region TEXT,
    raw_score DOUBLE PRECISION CHECK(raw_score BETWEEN 0 AND 100),
    purity DOUBLE PRECISION CHECK(purity BETWEEN 0 AND 100),
    direction TEXT,
    state TEXT NOT NULL,
    detail TEXT NOT NULL,
    method TEXT NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL,
    observed_at TIMESTAMPTZ,
    PRIMARY KEY(node_id,provider)
);
