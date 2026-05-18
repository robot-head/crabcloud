CREATE VIRTUAL TABLE oc_search USING fts5 (
    viewer_uid UNINDEXED,
    fileid     UNINDEXED,
    storage_id UNINDEXED,
    basename,
    path,
    mime       UNINDEXED,
    mtime      UNINDEXED,
    size       UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
