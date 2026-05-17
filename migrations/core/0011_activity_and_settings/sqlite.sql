CREATE TABLE oc_activity (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    affected_user   VARCHAR(64)  NOT NULL,
    actor           VARCHAR(64)  NOT NULL,
    event_type      VARCHAR(64)  NOT NULL,
    subject_id      VARCHAR(128) NOT NULL,
    subject_params  TEXT         NOT NULL,
    object_type     VARCHAR(32)  NOT NULL,
    object_id       BIGINT       NULL,
    occurred_at     BIGINT       NOT NULL,
    last_seen_at    BIGINT       NOT NULL,
    count           INTEGER      NOT NULL DEFAULT 1
);

CREATE INDEX idx_activity_user_time ON oc_activity (affected_user, occurred_at DESC);
CREATE INDEX idx_activity_coalesce  ON oc_activity (affected_user, actor, event_type, object_id, last_seen_at);

CREATE TABLE oc_activity_settings (
    user_id     VARCHAR(64) NOT NULL,
    event_type  VARCHAR(64) NOT NULL,
    stream      BOOLEAN     NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, event_type)
);
