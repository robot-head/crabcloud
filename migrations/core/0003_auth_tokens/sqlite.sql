CREATE TABLE oc_authtoken (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    uid               TEXT    NOT NULL,
    login_name        TEXT    NOT NULL,
    password          TEXT,
    name              TEXT    NOT NULL,
    token             TEXT    NOT NULL,
    type              INTEGER NOT NULL DEFAULT 0,
    remember          INTEGER NOT NULL DEFAULT 0,
    last_activity     INTEGER NOT NULL DEFAULT 0,
    last_check        INTEGER NOT NULL DEFAULT 0,
    public_key        TEXT,
    private_key       TEXT,
    version           INTEGER NOT NULL DEFAULT 2,
    scope             TEXT,
    expires           INTEGER,
    password_invalid  INTEGER NOT NULL DEFAULT 0,
    remote_wipe       INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX oc_authtoken_token_idx     ON oc_authtoken(token);
CREATE        INDEX oc_authtoken_uid_type_idx  ON oc_authtoken(uid, type);
CREATE        INDEX oc_authtoken_activity_idx  ON oc_authtoken(last_activity);
