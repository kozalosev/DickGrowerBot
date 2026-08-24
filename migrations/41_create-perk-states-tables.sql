-- The storage every perk gets. A perk owns the shape of its own `state` and nothing else reads it,
-- so adding a perk needs a row in Perks rather than a migration.
-- The name is read back into a validated domain type, and the constraint is what makes that read
-- unable to fail. It says exactly what `perk_name_validator` says; the test
-- `the_column_refuses_what_the_type_refuses` fails when the two drift apart.
CREATE TABLE IF NOT EXISTS Perks (
    id   smallserial PRIMARY KEY,
    name varchar(32) NOT NULL UNIQUE CHECK (name ~ '^[[:ascii:]]+$')
);

-- The key order mirrors the primary key of Dicks, so one range scan brings back every perk's state
-- for a dick.
CREATE TABLE IF NOT EXISTS Perk_States (
    chat_id bigint   NOT NULL REFERENCES Chats(id),
    uid     bigint   NOT NULL REFERENCES Users(uid) ON DELETE CASCADE,
    perk_id smallint NOT NULL REFERENCES Perks(id),
    state   jsonb    NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (chat_id, uid, perk_id)
);

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
    DELETE FROM Perk_States            WHERE uid = p_uid;
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
