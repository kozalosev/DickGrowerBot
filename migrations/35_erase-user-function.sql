-- Helpers for the owner to answer a data deletion request by hand. They are called from a DB
-- client, never from the bot:
--
--     SELECT erase_user(123456789);        -- delete everything, block for 90 days
--     SELECT erase_user(123456789, 30);    -- same, but for 30 days
--     SELECT ban_user(123456789, 7);       -- block only, keep the data
--     SELECT unban_user(123456789);        -- let the user play again
--
-- The Users row always survives. It carries the ban, and keeping it means no foreign key from
-- Loans, Dick_of_Day, Promo_Code_Activations or Stale_Dick_Shrinks is ever violated.

-- Deletes every row the user owns and blocks them for p_ban_days.
--
-- The name is cleared and created_at is moved to now: nothing is left to tell who the person was,
-- and a user who comes back after the ban starts with a fresh grace period.
CREATE OR REPLACE FUNCTION erase_user(p_uid bigint, p_ban_days int DEFAULT 90)
    RETURNS void
    LANGUAGE PLPGSQL
AS $$
DECLARE
    deleted int := 0;
    affected int;
BEGIN
    IF p_ban_days < 0 THEN
        RAISE EXCEPTION 'the ban length must not be negative, got %', p_ban_days;
    END IF;

    IF NOT EXISTS (SELECT 1 FROM Users WHERE uid = p_uid) THEN
        RAISE EXCEPTION 'there is no user with uid = %', p_uid;
    END IF;

    -- Every table that keeps rows owned by a user. A new one must be added here as well;
    -- the test `erase_user_covers_every_table_with_a_uid` fails when it isn't.
    DELETE FROM Dicks                  WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Battle_Stats           WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Loans                  WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Promo_Code_Activations WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Stale_Dick_Shrinks     WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Imports                WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Dick_of_Day            WHERE winner_uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;

    UPDATE Users
       SET name         = '',
           created_at   = current_timestamp,
           banned_until = current_timestamp + make_interval(days => p_ban_days)
     WHERE uid = p_uid;

    RAISE NOTICE 'erased the user %: % rows deleted, banned for % days', p_uid, deleted, p_ban_days;
END
$$;

-- Blocks the user without touching any of their data.
CREATE OR REPLACE FUNCTION ban_user(p_uid bigint, p_ban_days int DEFAULT 90)
    RETURNS void
    LANGUAGE PLPGSQL
AS $$
BEGIN
    IF p_ban_days < 0 THEN
        RAISE EXCEPTION 'the ban length must not be negative, got %', p_ban_days;
    END IF;

    UPDATE Users
       SET banned_until = current_timestamp + make_interval(days => p_ban_days)
     WHERE uid = p_uid;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'there is no user with uid = %', p_uid;
    END IF;
END
$$;

-- Lifts the ban. The data deleted by erase_user does not come back.
CREATE OR REPLACE FUNCTION unban_user(p_uid bigint)
    RETURNS void
    LANGUAGE PLPGSQL
AS $$
BEGIN
    UPDATE Users SET banned_until = NULL WHERE uid = p_uid;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'there is no user with uid = %', p_uid;
    END IF;
END
$$;
