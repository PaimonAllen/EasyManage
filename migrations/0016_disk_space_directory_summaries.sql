-- Intentionally removes historical per-file metadata. Back up before upgrade.
-- Rebuild only scan storage so the old heap and all four indexes are physically
-- released at commit; DELETE alone would leave their disk allocation behind.
SET LOCAL lock_timeout = '2s';
LOCK TABLE disk_space_scans, disk_space_entries IN ACCESS EXCLUSIVE MODE;

CREATE TABLE disk_space_directories (
    scan_id UUID NOT NULL REFERENCES disk_space_scans(id) ON DELETE CASCADE,
    id BIGINT NOT NULL CHECK (id >= 0),
    parent BIGINT CHECK (parent >= 0 AND parent < id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind = 'directory'),
    allocated BIGINT NOT NULL CHECK (allocated >= 0),
    apparent BIGINT NOT NULL CHECK (apparent >= 0),
    files BIGINT NOT NULL CHECK (files >= 0),
    directories BIGINT NOT NULL CHECK (directories >= 0),
    issue TEXT,
    incomplete BOOLEAN NOT NULL,
    mtime BIGINT NOT NULL,
    scan_path TEXT,
    PRIMARY KEY(scan_id, id)
);

-- Completed/partial/cancelled snapshots retain original directory IDs, totals,
-- identity, scope and timestamps. Incomplete jobs without a final result keep
-- their task record, but cannot expose a consistent directory tree.
INSERT INTO disk_space_directories
SELECT e.* FROM disk_space_entries e JOIN disk_space_scans s ON s.id=e.scan_id
WHERE e.kind='directory' AND s.result IS NOT NULL;

-- Also convert pre-streaming JSON snapshots. File names never survive in JSON.
INSERT INTO disk_space_directories
SELECT s.id, n.id, n.parent, n.name, n.kind, n.allocated, n.apparent,
       n.files, n.directories, n.issue, n.incomplete, n.mtime, n.scan_path
FROM disk_space_scans s
CROSS JOIN LATERAL jsonb_to_recordset(COALESCE(s.result->'nodes', '[]'::jsonb))
    AS n(id BIGINT, parent BIGINT, name TEXT, kind TEXT, allocated BIGINT,
         apparent BIGINT, files BIGINT, directories BIGINT, issue TEXT,
         incomplete BOOLEAN, mtime BIGINT, scan_path TEXT)
WHERE n.kind='directory'
ON CONFLICT(scan_id,id) DO NOTHING;

-- One parent lookup index serves pagination, aggregation and sorting. Size,
-- apparent size and name sorting now operate on child directories only.
CREATE INDEX disk_space_directory_children ON disk_space_directories(scan_id,parent);

UPDATE disk_space_scans s SET result =
    (s.result - 'nodes') || jsonb_build_object(
        'nodes', jsonb_build_array(to_jsonb(root)-'scan_id'),
        'paged', true, 'detail_level', 'directories',
        'saved_directories', totals.n)
FROM disk_space_directories root
JOIN (SELECT scan_id, COUNT(*) AS n FROM disk_space_directories GROUP BY scan_id) totals
    ON totals.scan_id=root.scan_id
WHERE root.id=0 AND s.id=root.scan_id AND s.result IS NOT NULL;

DROP TABLE disk_space_entries;
ANALYZE disk_space_directories;
