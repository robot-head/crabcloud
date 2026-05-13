CREATE TABLE oc_share (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    share_type    SMALLINT     NOT NULL,
    share_with    VARCHAR(255) NULL,
    uid_owner     VARCHAR(64)  NOT NULL,
    uid_initiator VARCHAR(64)  NOT NULL,
    parent        BIGINT       NULL,
    item_type     VARCHAR(64)  NOT NULL,
    item_source   BIGINT       NOT NULL,
    file_source   BIGINT       NOT NULL,
    file_target   VARCHAR(512) NOT NULL,
    permissions   INTEGER      NOT NULL,
    stime         BIGINT       NOT NULL,
    accepted      SMALLINT     NOT NULL DEFAULT 1,
    expiration    TIMESTAMP    NULL,
    token         VARCHAR(32)  NULL,
    password      VARCHAR(255) NULL,
    mail_send     SMALLINT     NOT NULL DEFAULT 0
);

CREATE        INDEX idx_share_with        ON oc_share (share_with, share_type);
CREATE        INDEX idx_share_owner       ON oc_share (uid_owner);
CREATE        INDEX idx_share_item_source ON oc_share (item_source);
CREATE UNIQUE INDEX idx_share_token       ON oc_share (token) WHERE token IS NOT NULL;
