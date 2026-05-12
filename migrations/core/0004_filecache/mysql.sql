CREATE TABLE oc_storages (
    numeric_id    INT          UNSIGNED NOT NULL AUTO_INCREMENT,
    id            VARCHAR(64)           NOT NULL,
    available     TINYINT      UNSIGNED NOT NULL DEFAULT 1,
    last_checked  INT          UNSIGNED NULL,
    PRIMARY KEY (numeric_id),
    UNIQUE KEY oc_storages_id_uniq (id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE TABLE oc_mimetypes (
    id        INT          UNSIGNED NOT NULL AUTO_INCREMENT,
    mimetype  VARCHAR(255)          NOT NULL,
    PRIMARY KEY (id),
    UNIQUE KEY oc_mimetypes_mimetype_uniq (mimetype)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE TABLE oc_filecache (
    fileid         BIGINT       UNSIGNED NOT NULL AUTO_INCREMENT,
    storage        INT          UNSIGNED NOT NULL,
    path           VARCHAR(4000)          NOT NULL,
    path_hash      CHAR(32)               NOT NULL,
    parent         BIGINT       UNSIGNED NULL,
    name           VARCHAR(250)           NOT NULL,
    mimetype       INT          UNSIGNED NOT NULL,
    mimepart       INT          UNSIGNED NOT NULL,
    size           BIGINT                 NOT NULL DEFAULT 0,
    mtime          INT          UNSIGNED NOT NULL DEFAULT 0,
    storage_mtime  INT          UNSIGNED NOT NULL DEFAULT 0,
    encrypted      TINYINT      UNSIGNED NOT NULL DEFAULT 0,
    etag           VARCHAR(40)            NOT NULL,
    permissions    INT          UNSIGNED NOT NULL DEFAULT 0,
    checksum       VARCHAR(255)           NULL,
    PRIMARY KEY (fileid),
    UNIQUE KEY fs_storage_path  (storage, path_hash),
    KEY        fs_parent        (parent),
    KEY        fs_mimepart      (mimepart),
    KEY        fs_mimetype      (mimetype),
    KEY        fs_storage_size  (storage, size),
    CONSTRAINT oc_filecache_storage_fk  FOREIGN KEY (storage)  REFERENCES oc_storages(numeric_id) ON DELETE CASCADE,
    CONSTRAINT oc_filecache_mimetype_fk FOREIGN KEY (mimetype) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    CONSTRAINT oc_filecache_mimepart_fk FOREIGN KEY (mimepart) REFERENCES oc_mimetypes(id)         ON DELETE RESTRICT,
    CONSTRAINT oc_filecache_parent_fk   FOREIGN KEY (parent)   REFERENCES oc_filecache(fileid)     ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
