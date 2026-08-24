-- The bot keeps the (tiny) ban list in memory, and a ban is written straight into the database by
-- the owner, so the bot had no way of hearing about one until its timer came round. Postgres can
-- say so itself.
--
-- A trigger rather than a line in each of erase_user, ban_user and unban_user: those bodies would
-- then have to be repeated here and kept in step for ever, and a ban applied by a plain UPDATE
-- would still go unheard.
--
-- pg_notify is delivered on commit, so a listener never learns of a ban that was rolled back.
CREATE OR REPLACE FUNCTION notify_ban_change()
    RETURNS trigger
    LANGUAGE PLPGSQL
AS $$
BEGIN
    IF NEW.banned_until IS DISTINCT FROM OLD.banned_until THEN
        PERFORM pg_notify('bans', NEW.uid::text);
    END IF;
    RETURN NULL;
END
$$;

CREATE OR REPLACE TRIGGER users_ban_changed
    AFTER UPDATE OF banned_until ON Users
    FOR EACH ROW
    EXECUTE FUNCTION notify_ban_change();
