-- Persistent platform network probe controls. Existing per-profile choices stay intact.
SET LOCAL lock_timeout='2s';
CREATE TABLE IF NOT EXISTS proxy_lab_monitoring (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK(singleton),
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    local_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1,
    global_epoch BIGINT NOT NULL DEFAULT 1,
    local_epoch BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
INSERT INTO proxy_lab_monitoring(singleton) VALUES(TRUE) ON CONFLICT DO NOTHING;
