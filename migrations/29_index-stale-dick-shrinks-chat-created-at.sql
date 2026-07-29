-- no-transaction
-- Backs the inline `shrinks` command's day-navigation MAX/MIN lookups (nearest older/newer date
-- with logged shrinks). Stale_Dick_Shrinks' PK is (chat_id, uid, created_at), so uid sitting
-- between chat_id and created_at means those lookups could only use the chat_id prefix and would
-- then scan every row of the chat; this index lets them seek straight to the answer instead.
--
-- Kept in its own migration rather than appended to migration 28: sqlx executes a `-- no-transaction`
-- migration's whole SQL text as one query string, and Postgres treats a multi-statement string as
-- an implicit transaction block unless it contains explicit BEGIN/COMMIT — which CREATE INDEX
-- CONCURRENTLY refuses to run inside, even with the no-transaction directive set on the file.
CREATE INDEX CONCURRENTLY IF NOT EXISTS stale_dick_shrinks_idx_chat_created_at
    ON Stale_Dick_Shrinks(chat_id, created_at DESC);
