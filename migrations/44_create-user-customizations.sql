-- Only the active visual customization is stored here. Ownership, acquisition and payment belong
-- to their own future component and must decide whether to call the repository's set method.
CREATE TABLE IF NOT EXISTS User_Customizations (
    uid              bigint      PRIMARY KEY REFERENCES Users(uid) ON DELETE CASCADE,
    customization_id varchar(64) NOT NULL,
    updated_at       timestamptz NOT NULL DEFAULT current_timestamp
);

-- Deletes every row the user owns and blocks them for p_ban_days. ON DELETE CASCADE does not help
-- here because erasure deliberately keeps the anonymized Users row so the temporary ban survives.
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
    DELETE FROM User_Customizations    WHERE uid = p_uid;
    GET DIAGNOSTICS affected = ROW_COUNT; deleted := deleted + affected;

    UPDATE Users
       SET name         = '',
           created_at   = current_timestamp,
           banned_until = current_timestamp + make_interval(days => p_ban_days)
     WHERE uid = p_uid;

    RAISE NOTICE 'erased the user %: % rows deleted, banned for % days', p_uid, deleted, p_ban_days;
END
$$;
