-- Narrows the index claim_due seeks in to the rows it can actually claim. The index this replaces
-- (migration 37) covers every row of the table, finished ones included, so it keeps growing with the
-- history the cleaner holds for MSG_SELFDESTRUCT_TABLE_CLEANING_DELAY -- while the claim only ever
-- looks at `finished_at IS NULL`. The broadcast queue's twin in migration 38 is partial from the
-- start; this is the same shape, and it also serves the count behind self_destruction_pending.
--
-- Both statements share one transaction, so there is no moment at which the claim has no index to
-- seek in. That costs a lock on Scheduled_Message_Deletions for as long as the build takes, which a
-- single instance can afford to wait out.
CREATE INDEX IF NOT EXISTS scheduled_message_deletions_idx_fire_after_pending
    ON Scheduled_Message_Deletions (fire_after) WHERE finished_at IS NULL;

DROP INDEX IF EXISTS Scheduled_Message_Deletions_fire_after_idx;
