SET LOCAL lock_timeout = '2s';
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS display_name TEXT;
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS management_note TEXT NOT NULL DEFAULT '';
ALTER TABLE nodes ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;

-- Short-lived, single-node enrollment secrets; never send the platform secret.
CREATE TABLE machine_enrollments (
    node_id UUID PRIMARY KEY REFERENCES nodes(id),
    token_hash CHAR(64) NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    expected_user TEXT NOT NULL
);
