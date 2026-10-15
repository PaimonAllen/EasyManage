-- Operator annotations only; automated Agent findings remain unchanged.
CREATE TABLE permission_reviews (
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    category TEXT NOT NULL CHECK (length(category) <= 32),
    title TEXT NOT NULL CHECK (octet_length(title) <= 256),
    path TEXT NOT NULL CHECK (octet_length(path) <= 1024),
    evidence JSONB NOT NULL,
    note TEXT NOT NULL DEFAULT '' CHECK (length(note) <= 1000),
    marked_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (node_id, category, title, path)
);
