CREATE TABLE oc_appconfig (
    appid       VARCHAR(32)    NOT NULL,
    configkey   VARCHAR(64)    NOT NULL,
    configvalue LONGTEXT       NOT NULL,
    PRIMARY KEY (appid, configkey)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE INDEX appconfig_appid_idx ON oc_appconfig(appid);
