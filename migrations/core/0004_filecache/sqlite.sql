CREATE TABLE oc_storages (
    numeric_id    INTEGER PRIMARY KEY AUTOINCREMENT,
    id            TEXT    NOT NULL UNIQUE,
    available     INTEGER NOT NULL DEFAULT 1,
    last_checked  INTEGER NULL
);

CREATE TABLE oc_mimetypes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    mimetype  TEXT    NOT NULL UNIQUE
);

CREATE TABLE oc_filecache (
    fileid         INTEGER PRIMARY KEY AUTOINCREMENT,
    storage        INTEGER NOT NULL,
    path           TEXT    NOT NULL,
    path_hash      TEXT    NOT NULL,
    parent         INTEGER NULL,
    name           TEXT    NOT NULL,
    mimetype       INTEGER NOT NULL,
    mimepart       INTEGER NOT NULL,
    size           INTEGER NOT NULL DEFAULT 0,
    mtime          INTEGER NOT NULL DEFAULT 0,
    storage_mtime  INTEGER NOT NULL DEFAULT 0,
    encrypted      INTEGER NOT NULL DEFAULT 0,
    etag           TEXT    NOT NULL,
    permissions    INTEGER NOT NULL DEFAULT 0,
    checksum       TEXT    NULL,
    FOREIGN KEY (storage)  REFERENCES oc_storages(numeric_id) ON DELETE CASCADE,
    FOREIGN KEY (mimetype) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    FOREIGN KEY (mimepart) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    FOREIGN KEY (parent)   REFERENCES oc_filecache(fileid)     ON DELETE CASCADE
);

CREATE UNIQUE INDEX fs_storage_path  ON oc_filecache (storage, path_hash);
CREATE        INDEX fs_parent        ON oc_filecache (parent);
CREATE        INDEX fs_mimepart      ON oc_filecache (mimepart);
CREATE        INDEX fs_mimetype      ON oc_filecache (mimetype);
CREATE        INDEX fs_storage_size  ON oc_filecache (storage, size);
