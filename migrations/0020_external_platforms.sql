-- Browser shortcuts are independent of Agent identities and monitoring data.
SET LOCAL lock_timeout = '2s';
CREATE TABLE IF NOT EXISTS external_platforms (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 128),
    url TEXT NOT NULL CHECK (octet_length(url) <= 4096 AND url ~ '^https?://'),
    note TEXT NOT NULL DEFAULT '' CHECK (char_length(note) <= 1000),
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision BETWEEN 1 AND 9007199254740991),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ
);
