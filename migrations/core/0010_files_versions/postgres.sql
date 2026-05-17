CREATE TABLE oc_files_versions (
    id             BIGSERIAL    PRIMARY KEY,
    storage_id     BIGINT       NOT NULL,
    fileid         BIGINT       NOT NULL,
    "user"         VARCHAR(64)  NOT NULL,
    path           VARCHAR(512) NOT NULL,
    version_mtime  BIGINT       NOT NULL,
    size           BIGINT       NOT NULL
);

CREATE INDEX idx_versions_user_fileid    ON oc_files_versions ("user", fileid);
CREATE INDEX idx_versions_user_mtime     ON oc_files_versions ("user", version_mtime);
CREATE INDEX idx_versions_storage_fileid ON oc_files_versions (storage_id, fileid);
