CREATE TABLE oc_activity (
    id              BIGINT       NOT NULL AUTO_INCREMENT PRIMARY KEY,
    affected_user   VARCHAR(64)  NOT NULL,
    actor           VARCHAR(64)  NOT NULL,
    event_type      VARCHAR(64)  NOT NULL,
    subject_id      VARCHAR(128) NOT NULL,
    subject_params  TEXT         NOT NULL,
    object_type     VARCHAR(32)  NOT NULL,
    object_id       BIGINT       NULL,
    occurred_at     BIGINT       NOT NULL,
    last_seen_at    BIGINT       NOT NULL,
    count           INT          NOT NULL DEFAULT 1,
    INDEX idx_activity_user_time (affected_user, occurred_at),
    INDEX idx_activity_coalesce  (affected_user, actor, event_type, object_id, last_seen_at)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

CREATE TABLE oc_activity_settings (
    user_id     VARCHAR(64)  NOT NULL,
    event_type  VARCHAR(64)  NOT NULL,
    stream      TINYINT(1)   NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, event_type)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;
