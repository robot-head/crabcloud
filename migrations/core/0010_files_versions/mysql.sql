CREATE TABLE oc_files_versions (
    id             BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
    storage_id     BIGINT       NOT NULL,
    fileid         BIGINT       NOT NULL,
    `user`         VARCHAR(64)  NOT NULL,
    path           VARCHAR(512) NOT NULL,
    version_mtime  BIGINT       NOT NULL,
    size           BIGINT       NOT NULL,
    INDEX idx_versions_user_fileid    (`user`, fileid),
    INDEX idx_versions_user_mtime     (`user`, version_mtime),
    INDEX idx_versions_storage_fileid (storage_id, fileid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
