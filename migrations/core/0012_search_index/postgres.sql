CREATE TABLE oc_search (
    viewer_uid  VARCHAR(64)  NOT NULL,
    fileid      BIGINT       NOT NULL,
    storage_id  VARCHAR(255) NOT NULL,
    basename    VARCHAR(255) NOT NULL,
    path        VARCHAR(512) NOT NULL,
    mime        VARCHAR(255) NOT NULL,
    mtime       BIGINT       NOT NULL,
    size        BIGINT       NOT NULL,
    tsv         tsvector     GENERATED ALWAYS AS (
                  to_tsvector('simple', basename || ' ' || path)
                ) STORED,
    PRIMARY KEY (viewer_uid, fileid)
);

CREATE INDEX idx_search_viewer       ON oc_search (viewer_uid);
CREATE INDEX idx_search_storage_path ON oc_search (storage_id, path);
CREATE INDEX idx_search_tsv          ON oc_search USING GIN (tsv);
