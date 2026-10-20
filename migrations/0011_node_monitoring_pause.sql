-- Preserve node identity/history/privileges; only change monitoring intent and ACK.
SET LOCAL lock_timeout = '2s';
ALTER TABLE nodes ADD COLUMN monitoring_paused BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE nodes ADD COLUMN monitoring_revision BIGINT NOT NULL DEFAULT 0 CHECK(monitoring_revision>=0);
ALTER TABLE nodes ADD COLUMN monitoring_applied_revision BIGINT;
ALTER TABLE nodes ADD COLUMN control_seen_at TIMESTAMPTZ;
