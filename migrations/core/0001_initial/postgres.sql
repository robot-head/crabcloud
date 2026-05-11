CREATE TABLE oc_appconfig (
    appid       VARCHAR(32)  NOT NULL,
    configkey   VARCHAR(64)  NOT NULL,
    configvalue TEXT         NOT NULL DEFAULT '',
    PRIMARY KEY (appid, configkey)
);

CREATE INDEX appconfig_appid_idx ON oc_appconfig(appid);
