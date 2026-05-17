CREATE TABLE oc_files_versions (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    storage_id     BIGINT       NOT NULL,
    fileid         BIGINT       NOT NULL,
    "user"         VARCHAR(64)  NOT NULL,
    path           VARCHAR(512) NOT NULL,
    version_mtime  BIGINT       NOT NULL,
    size           BIGINT       NOT NULL
);

CREATE INDEX idx_versions_user_fileid    ON oc_files_versions ("user", fileid);
CREATE INDEX idx_versions_user_mtime     ON oc_files_versions ("user", version_mtime);
CREATE INDEX idx_versions_storage_fileid ON oc_files_versions (storage_id, fileid);
-- Concurrent-writer-in-same-second guard. Two simultaneous writers to
-- the same (storage_id, fileid) would otherwise produce two rows with
-- identical `version_mtime`, both pointing at byte-identical `.v{ts}`
-- files on disk. `snapshot_if_needed` catches this UNIQUE violation
-- and treats it as a soft-skip (`Ok(None)`).
CREATE UNIQUE INDEX idx_versions_unique  ON oc_files_versions (storage_id, fileid, version_mtime);
