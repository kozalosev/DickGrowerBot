-- Keeps the planner's picture of Stale_Dick_Shrinks close enough to the truth to plan against.
--
-- The daily run inserts about 914k rows under one new date, and the broadcast worker reads them
-- minutes later. So the value every summary selects on is always the one the statistics know least
-- about: a date past the end of the histogram is estimated at a single row, and the planner then
-- picks a plan that is right for one row and ruinous for 914k -- joining the whole day's shrinks to
-- Users, Dicks and Chats a row at a time, seventeen seconds and eleven million buffers to answer a
-- page of ten. With honest statistics the same query is a bitmap lookup of the chat followed by a
-- seek on stale_dick_shrinks_idx_chat_created_at, and takes a fraction of a millisecond.
--
-- Autovacuum will not notice by itself: its threshold is 50 rows plus a tenth of the table, and a
-- night's insert is a smaller share of that with every month the table grows, so the statistics can
-- sit days out of date. A hundredth keeps one night above the line.
--
-- Autovacuum still decides when, and the summaries of a run's first batches are claimable before it
-- has had any reason to wake. last_autoanalyze in pg_stat_user_tables is what says whether it kept
-- up on a night that was slow.
ALTER TABLE Stale_Dick_Shrinks SET (autovacuum_analyze_scale_factor = 0.01);

-- The setting governs what happens next and nothing about statistics that are stale already, and
-- the table this lands on has been collecting a night's rows at a time under the old threshold. The
-- first run to touch it should not be the one to pay for that.
ANALYZE Stale_Dick_Shrinks;
