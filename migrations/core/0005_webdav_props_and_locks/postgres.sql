CREATE TABLE oc_properties (
    id             BIGSERIAL     PRIMARY KEY,
    userid         VARCHAR(64)   NOT NULL,
    propertypath   VARCHAR(4000) NOT NULL,
    propertyname   VARCHAR(255)  NOT NULL,
    propertyvalue  TEXT          NULL
);
CREATE        INDEX oc_properties_pathonly ON oc_properties (userid, propertypath);
CREATE UNIQUE INDEX oc_properties_pathname ON oc_properties (userid, propertypath, propertyname);

CREATE TABLE oc_filelocks (
    id     BIGSERIAL    PRIMARY KEY,
    key    VARCHAR(2048) NOT NULL UNIQUE,
    ttl    INTEGER      NOT NULL DEFAULT 86400,
    lock   INTEGER      NOT NULL DEFAULT 0,
    token  VARCHAR(255) NULL,
    scope  VARCHAR(32)  NULL,
    depth  VARCHAR(32)  NULL,
    owner  VARCHAR(2048) NULL
);
