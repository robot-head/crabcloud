CREATE TABLE oc_search (
    viewer_uid  VARCHAR(64)  NOT NULL,
    fileid      BIGINT       NOT NULL,
    storage_id  VARCHAR(255) NOT NULL,
    basename    VARCHAR(255) NOT NULL,
    path        VARCHAR(512) NOT NULL,
    mime        VARCHAR(255) NOT NULL,
    mtime       BIGINT       NOT NULL,
    size        BIGINT       NOT NULL,
    PRIMARY KEY (viewer_uid, fileid),
    INDEX idx_search_viewer        (viewer_uid),
    INDEX idx_search_storage_path  (storage_id, path),
    FULLTEXT INDEX ftx_search_text (basename, path)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
