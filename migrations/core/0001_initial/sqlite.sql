CREATE TABLE oc_appconfig (
    appid       TEXT    NOT NULL,
    configkey   TEXT    NOT NULL,
    configvalue TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (appid, configkey)
);

CREATE INDEX appconfig_appid_idx ON oc_appconfig(appid);
