CREATE TABLE oc_properties (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    userid         TEXT    NOT NULL,
    propertypath   TEXT    NOT NULL,
    propertyname   TEXT    NOT NULL,
    propertyvalue  TEXT    NULL
);
CREATE        INDEX oc_properties_pathonly ON oc_properties (userid, propertypath);
CREATE UNIQUE INDEX oc_properties_pathname ON oc_properties (userid, propertypath, propertyname);

CREATE TABLE oc_filelocks (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    key    TEXT    NOT NULL UNIQUE,
    ttl    INTEGER NOT NULL DEFAULT 86400,
    lock   INTEGER NOT NULL DEFAULT 0,
    token  TEXT    NULL,
    scope  TEXT    NULL,
    depth  TEXT    NULL,
    owner  TEXT    NULL
);
