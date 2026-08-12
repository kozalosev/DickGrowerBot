-- Three tables were being read by a column that led no index, and so were scanned in full every
-- time: the whole of Dicks to give one user their promo bonus or their statistics, the whole of
-- Loans and of Battle_Stats to merge two chats — the latter three times over, plus a fourth for the
-- cascade from Chats.
--
-- Where every query names both columns anyway, one index serves both shapes: equality on a pair is
-- matched whichever column comes first, which leaves the leading one free to be the chat, and the
-- separate index on the user is then redundant. That is why two of the tables come out of this with
-- one index fewer than they went in with. What it costs is the queries that filter by the user
-- alone, which is erasing a user — run by hand, on a request, and never by the bot.
--
-- Foreign keys are unaffected: an insert checks the key against the parent's own primary key, so it
-- never looks at these indexes. Only deleting a row from Users would, and the bot deletes none.
CREATE INDEX IF NOT EXISTS dicks_idx_uid ON Dicks(uid);

CREATE INDEX IF NOT EXISTS idx_loans_chat_id_uid ON Loans(chat_id, uid);
DROP INDEX IF EXISTS idx_loans_uid;

ALTER TABLE Battle_Stats DROP CONSTRAINT IF EXISTS battle_stats_pkey,
                         ADD PRIMARY KEY (chat_id, uid);

-- Meant for the daily scan for stale dicks, but staleness is not a rare property here: nine of
-- every ten positive dicks are overdue at any moment, so the condition selects almost the whole
-- table and the planner reaches for the chat instead. It is not free to keep, either — `updated_at`
-- changes on every growth, so each write moves the entry.
DROP INDEX IF EXISTS dicks_idx_updated_at;
