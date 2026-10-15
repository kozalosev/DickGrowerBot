-- A user's own settings, the same jsonb shape as Chats.settings. A private chat has no Chats row,
-- so this is where whatever a user sets for every chat at once is kept.
ALTER TABLE Users ADD COLUMN IF NOT EXISTS settings jsonb NOT NULL DEFAULT '{}'::jsonb;

-- What a user chose for one chat only. A key that is absent here falls back to Users.settings.
CREATE TABLE IF NOT EXISTS Player_Settings (
    chat_id  bigint NOT NULL REFERENCES Chats(id),
    uid      bigint NOT NULL REFERENCES Users(uid) ON DELETE CASCADE,
    settings jsonb  NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (chat_id, uid)
);

-- Deletes every row the user owns and blocks them for p_ban_days.
--
-- The name and the settings are cleared and created_at is moved to now: nothing is left to tell who
-- the person was, and a user who comes back after the ban starts with a fresh grace period.
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
    DELETE FROM Player_Settings        WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;
    DELETE FROM Dick_of_Day            WHERE winner_uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;

    UPDATE Users
       SET name         = '',
           settings     = '{}'::jsonb,
           created_at   = current_timestamp,
           banned_until = current_timestamp + make_interval(days => p_ban_days)
     WHERE uid = p_uid;

    RAISE NOTICE 'erased the user %: % rows deleted, banned for % days', p_uid, deleted, p_ban_days;
END
$$;
