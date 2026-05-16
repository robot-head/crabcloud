CREATE TABLE oc_mail_queue (
    id               BIGINT       NOT NULL AUTO_INCREMENT,
    recipient        VARCHAR(255) NOT NULL,
    subject          VARCHAR(512) NOT NULL,
    html_body        TEXT         NOT NULL,
    text_body        TEXT         NOT NULL,
    event_type       VARCHAR(64)  NOT NULL,
    attempts         INT          NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMP    NOT NULL,
    state            VARCHAR(16)  NOT NULL DEFAULT 'Pending',
    claimed_at       TIMESTAMP    NULL,
    last_error       TEXT         NULL,
    created_at       TIMESTAMP    NOT NULL,
    sent_at          TIMESTAMP    NULL,
    PRIMARY KEY (id),
    INDEX idx_mail_queue_state_next_attempt (state, next_attempt_at)
) ENGINE=InnoDB COLLATE=utf8mb4_bin;

CREATE TABLE oc_user_notification_prefs (
    user_id      VARCHAR(64)  NOT NULL,
    event_type   VARCHAR(64)  NOT NULL,
    enabled      SMALLINT     NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, event_type)
) ENGINE=InnoDB COLLATE=utf8mb4_bin;
