CREATE OR REPLACE FUNCTION check_and_update_dicks_timestamp()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    IF current_date = date(OLD.updated_at) AND NEW.bonus_attempts = 0 THEN
        RAISE EXCEPTION 'Your dick has been already grown today!'
            USING ERRCODE = 'GD0E1';
    END IF;

    IF NEW.bonus_attempts > 0 THEN
        NEW.bonus_attempts := NEW.bonus_attempts - 1;
    END IF;

    RETURN NEW;
END
$$;
