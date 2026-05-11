CREATE TABLE oc_users (
    uid          VARCHAR(64)  NOT NULL,
    password     LONGTEXT,
    displayname  VARCHAR(64),
    email        VARCHAR(255),
    last_seen    BIGINT  NOT NULL DEFAULT 0,
    enabled      TINYINT NOT NULL DEFAULT 1,
    PRIMARY KEY (uid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX oc_users_email_idx ON oc_users(email);

CREATE TABLE oc_groups (
    gid          VARCHAR(64) NOT NULL,
    displayname  VARCHAR(64),
    PRIMARY KEY (gid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE oc_group_user (
    gid  VARCHAR(64) NOT NULL,
    uid  VARCHAR(64) NOT NULL,
    PRIMARY KEY (gid, uid)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX oc_group_user_uid_idx ON oc_group_user(uid);

CREATE TABLE oc_preferences (
    userid       VARCHAR(64) NOT NULL,
    appid        VARCHAR(32) NOT NULL,
    configkey    VARCHAR(64) NOT NULL,
    configvalue  LONGTEXT,
    PRIMARY KEY (userid, appid, configkey)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX oc_preferences_appid_idx ON oc_preferences(appid);

INSERT INTO oc_groups (gid, displayname) VALUES ('admin', 'Admin');
