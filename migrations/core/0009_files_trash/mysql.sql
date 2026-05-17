CREATE TABLE oc_files_trash (
    id             BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
    `user`         VARCHAR(64)  NOT NULL,
    basename       VARCHAR(255) NOT NULL,
    suffix         VARCHAR(32)  NOT NULL,
    location       VARCHAR(512) NOT NULL,
    deleted_at     BIGINT       NOT NULL,
    type           VARCHAR(16)  NOT NULL,
    fileid_legacy  BIGINT       NULL,
    INDEX        idx_trash_user_deleted (`user`, deleted_at),
    UNIQUE INDEX idx_trash_user_name    (`user`, basename, suffix)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
