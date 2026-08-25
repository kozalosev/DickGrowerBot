-- no-transaction
-- Backs get_last_shrink_timestamp's unqualified `SELECT max(created_at)`, which publishes the
-- daily_shrink_last_run_timestamp_seconds liveness gauge. The chat_id-led index from migration 29
-- can't serve it: MAX(created_at) only turns into a backward index scan when created_at is an
-- index's leading column, and this query has no chat_id to seek with. Without this index Postgres
-- scans the whole table, which grows daily and so does the scan.
CREATE INDEX CONCURRENTLY IF NOT EXISTS stale_dick_shrinks_idx_created_at
    ON Stale_Dick_Shrinks(created_at DESC);
