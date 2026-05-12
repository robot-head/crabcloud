CREATE TABLE oc_storages (
    numeric_id    SERIAL       PRIMARY KEY,
    id            VARCHAR(64)  NOT NULL UNIQUE,
    available     SMALLINT     NOT NULL DEFAULT 1,
    last_checked  INTEGER      NULL
);

CREATE TABLE oc_mimetypes (
    id        SERIAL        PRIMARY KEY,
    mimetype  VARCHAR(255)  NOT NULL UNIQUE
);

CREATE TABLE oc_filecache (
    fileid         BIGSERIAL     PRIMARY KEY,
    storage        INTEGER       NOT NULL REFERENCES oc_storages(numeric_id) ON DELETE CASCADE,
    path           VARCHAR(4000) NOT NULL,
    path_hash      CHAR(32)      NOT NULL,
    parent         BIGINT        NULL REFERENCES oc_filecache(fileid) ON DELETE CASCADE,
    name           VARCHAR(250)  NOT NULL,
    mimetype       INTEGER       NOT NULL REFERENCES oc_mimetypes(id) ON DELETE RESTRICT,
    mimepart       INTEGER       NOT NULL REFERENCES oc_mimetypes(id) ON DELETE RESTRICT,
    size           BIGINT        NOT NULL DEFAULT 0,
    mtime          INTEGER       NOT NULL DEFAULT 0,
    storage_mtime  INTEGER       NOT NULL DEFAULT 0,
    encrypted      SMALLINT      NOT NULL DEFAULT 0,
    etag           VARCHAR(40)   NOT NULL,
    permissions    INTEGER       NOT NULL DEFAULT 0,
    checksum       VARCHAR(255)  NULL
);

CREATE UNIQUE INDEX fs_storage_path  ON oc_filecache (storage, path_hash);
CREATE        INDEX fs_parent        ON oc_filecache (parent);
CREATE        INDEX fs_mimepart      ON oc_filecache (mimepart);
CREATE        INDEX fs_mimetype      ON oc_filecache (mimetype);
CREATE        INDEX fs_storage_size  ON oc_filecache (storage, size);
