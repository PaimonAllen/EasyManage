-- Additive settings and nullable sampling timestamp only. Never rewrite historical rows.
SET LOCAL lock_timeout = '2s';
CREATE TABLE collection_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    policy JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
ALTER TABLE metrics_samples
    ADD COLUMN IF NOT EXISTS gpu_collected_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS fast_collected_at TIMESTAMPTZ;
