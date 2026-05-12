CREATE TABLE oc_authtoken (
    id                BIGSERIAL    PRIMARY KEY,
    uid               VARCHAR(64)  NOT NULL,
    login_name        VARCHAR(64)  NOT NULL,
    password          TEXT,
    name              VARCHAR(128) NOT NULL,
    token             VARCHAR(200) NOT NULL,
    type              SMALLINT     NOT NULL DEFAULT 0,
    remember          SMALLINT     NOT NULL DEFAULT 0,
    last_activity     BIGINT       NOT NULL DEFAULT 0,
    last_check        BIGINT       NOT NULL DEFAULT 0,
    public_key        TEXT,
    private_key       TEXT,
    version           SMALLINT     NOT NULL DEFAULT 2,
    scope             TEXT,
    expires           BIGINT,
    password_invalid  SMALLINT     NOT NULL DEFAULT 0,
    remote_wipe       SMALLINT     NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX oc_authtoken_token_idx     ON oc_authtoken(token);
CREATE        INDEX oc_authtoken_uid_type_idx  ON oc_authtoken(uid, type);
CREATE        INDEX oc_authtoken_activity_idx  ON oc_authtoken(last_activity);
