-- The bot keeps the list of bans in memory and re-reads it on a timer, so for a few minutes after
-- a ban it may still let the user through. This trigger is what actually holds the line.
--
-- Users is the only place it has to sit. Both ways back into the game — /grow and the inline
-- handler — start with the same upsert, and its `DO UPDATE SET name = $2` is exactly what would
-- bring an erased name back. Everything else needs rows that erase_user has already deleted.
CREATE OR REPLACE FUNCTION forbid_updates_of_banned_users()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    -- A statement that changes banned_until is erase_user, ban_user or unban_user doing their job;
    -- anything else touching a banned user's row is the bot trying to take them back.
    IF OLD.banned_until > current_timestamp AND NEW.banned_until IS NOT DISTINCT FROM OLD.banned_until THEN
        RAISE EXCEPTION '%', to_char(OLD.banned_until AT TIME ZONE 'UTC', 'DD.MM.YYYY')
            USING ERRCODE = 'GD3E1';
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE TRIGGER trg_forbid_updates_of_banned_users BEFORE UPDATE ON Users
    FOR EACH ROW EXECUTE FUNCTION forbid_updates_of_banned_users();
