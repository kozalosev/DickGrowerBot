CREATE OR REPLACE FUNCTION check_dod_timestamp()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
DECLARE
    dod_name varchar;
BEGIN
    SELECT name INTO dod_name FROM Dick_of_Day dod
        JOIN Users u ON dod.winner_uid = u.uid
        WHERE created_at = current_date AND chat_id = NEW.chat_id;
    IF dod_name IS NOT NULL THEN
        RAISE EXCEPTION '%', dod_name
            USING ERRCODE = 'GD0E2';
    END IF;

    NEW.created_at := current_timestamp;
    RETURN NEW;
END
$$;
