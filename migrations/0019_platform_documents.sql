-- Shared editable references are independent of monitoring and Agent data.
SET LOCAL lock_timeout = '2s';
CREATE TABLE IF NOT EXISTS platform_documents (
    slug TEXT PRIMARY KEY,
    content TEXT NOT NULL CHECK (octet_length(content) <= 1048576),
    revision BIGINT NOT NULL CHECK (revision > 0 AND revision <= 9007199254740991),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
