SET LOCAL lock_timeout = '2s';
-- Stream new snapshots into rows; existing JSON snapshots remain readable.
CREATE TABLE disk_space_entries (
    scan_id UUID NOT NULL REFERENCES disk_space_scans(id) ON DELETE CASCADE,
    id BIGINT NOT NULL CHECK (id >= 0),
    parent BIGINT CHECK (parent >= 0 AND parent < id),
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('directory','file','symlink','special')),
    allocated BIGINT NOT NULL CHECK (allocated >= 0),
    apparent BIGINT NOT NULL CHECK (apparent >= 0),
    files BIGINT NOT NULL CHECK (files >= 0),
    directories BIGINT NOT NULL CHECK (directories >= 0),
    issue TEXT,
    incomplete BOOLEAN NOT NULL,
    mtime BIGINT NOT NULL,
    PRIMARY KEY(scan_id,id)
);
CREATE INDEX disk_space_children_size ON disk_space_entries(scan_id,parent,allocated DESC,id);
CREATE INDEX disk_space_children_apparent ON disk_space_entries(scan_id,parent,apparent DESC,id);
CREATE INDEX disk_space_children_name ON disk_space_entries(scan_id,parent,name,id);
