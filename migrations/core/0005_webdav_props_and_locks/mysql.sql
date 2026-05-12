CREATE TABLE oc_properties (
    id             BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    userid         VARCHAR(64)     NOT NULL,
    propertypath   VARCHAR(4000)   NOT NULL,
    propertyname   VARCHAR(255)    NOT NULL,
    propertyvalue  LONGTEXT        NULL,
    PRIMARY KEY (id),
    KEY        oc_properties_pathonly (userid, propertypath),
    UNIQUE KEY oc_properties_pathname (userid, propertypath, propertyname(191))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;

CREATE TABLE oc_filelocks (
    id     BIGINT UNSIGNED NOT NULL AUTO_INCREMENT,
    `key`  VARCHAR(2048)   NOT NULL,
    ttl    INT             NOT NULL DEFAULT 86400,
    `lock` INT             NOT NULL DEFAULT 0,
    token  VARCHAR(255)    NULL,
    scope  VARCHAR(32)     NULL,
    depth  VARCHAR(32)     NULL,
    owner  VARCHAR(2048)   NULL,
    PRIMARY KEY (id),
    UNIQUE KEY oc_filelocks_key (`key`(255))
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin;
