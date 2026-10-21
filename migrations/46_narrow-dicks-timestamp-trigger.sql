-- Scopes the once-a-day guard to the statements it is actually about.
--
-- The trigger fires for every row of every UPDATE on Dicks, PL/pgSQL body and all, and the daily
-- shrink updates about 1.3M of them in one night. Yet only a growth is subject to the rule the body
-- enforces, and a growth is exactly the statement that sets updated_at: create_or_grow is the only
-- write in the bot that touches that column on purpose. A column list is checked once per
-- statement, so the shrink, the promo codes, the import and the two length adjustments no longer
-- reach the function at all.
--
-- Those five carried `bonus_attempts + 1` for no other reason than to satisfy the body, which
-- subtracted the same one straight back; they drop it in the same commit as this.
--
-- Only the UPDATE half is narrowed. On INSERT there is no OLD row, so the day is never the same one
-- and the guard cannot fire — but the decrement below it can, and both the chat merge and
-- create_or_grow are written around its doing so. Leaving INSERT alone keeps every one of those
-- statements behaving exactly as it did.
CREATE OR REPLACE TRIGGER trg_check_and_update_dicks_timestamp
    BEFORE INSERT OR UPDATE OF updated_at ON Dicks
    FOR EACH ROW EXECUTE FUNCTION check_and_update_dicks_timestamp();
