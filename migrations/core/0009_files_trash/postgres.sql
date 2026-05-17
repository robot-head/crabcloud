CREATE TABLE oc_files_trash (
    id             BIGSERIAL    PRIMARY KEY,
    "user"         VARCHAR(64)  NOT NULL,
    basename       VARCHAR(255) NOT NULL,
    suffix         VARCHAR(32)  NOT NULL,
    location       VARCHAR(512) NOT NULL,
    deleted_at     BIGINT       NOT NULL,
    type           VARCHAR(16)  NOT NULL,
    fileid_legacy  BIGINT       NULL
);

CREATE        INDEX idx_trash_user_deleted ON oc_files_trash ("user", deleted_at);
CREATE UNIQUE INDEX idx_trash_user_name    ON oc_files_trash ("user", basename, suffix);
