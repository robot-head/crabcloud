CREATE TABLE oc_users (
    uid          TEXT    NOT NULL,
    password     TEXT,
    displayname  TEXT,
    email        TEXT,
    last_seen    INTEGER NOT NULL DEFAULT 0,
    enabled      INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (uid)
);
CREATE UNIQUE INDEX oc_users_email_idx ON oc_users(email) WHERE email IS NOT NULL;

CREATE TABLE oc_groups (
    gid          TEXT NOT NULL,
    displayname  TEXT,
    PRIMARY KEY (gid)
);

CREATE TABLE oc_group_user (
    gid  TEXT NOT NULL,
    uid  TEXT NOT NULL,
    PRIMARY KEY (gid, uid)
);
CREATE INDEX oc_group_user_uid_idx ON oc_group_user(uid);

CREATE TABLE oc_preferences (
    userid       TEXT NOT NULL,
    appid        TEXT NOT NULL,
    configkey    TEXT NOT NULL,
    configvalue  TEXT,
    PRIMARY KEY (userid, appid, configkey)
);
CREATE INDEX oc_preferences_appid_idx ON oc_preferences(appid);

INSERT INTO oc_groups (gid, displayname) VALUES ('admin', 'Admin');
