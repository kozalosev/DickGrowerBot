CREATE TABLE IF NOT EXISTS Dick_of_Day (
    chat_id bigint,
    winner_uid bigint NOT NULL REFERENCES Users(uid),
    created_at date DEFAULT current_date,

    PRIMARY KEY (chat_id, created_at)
);

ALTER TABLE Dicks ADD COLUMN IF NOT EXISTS bonus_attempts integer NOT NULL DEFAULT 0;

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

    NEW.updated_at := current_timestamp;
    RETURN NEW;
END
$$;

CREATE OR REPLACE FUNCTION check_dod_timestamp()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
DECLARE
    dod_name varchar;
BEGIN
    SELECT name INTO dod_name FROM Dick_of_Day dod
        JOIN Users u ON dod.winner_uid = u.uid
        WHERE created_at = current_date;
    IF dod_name IS NOT NULL THEN
        RAISE EXCEPTION '%', dod_name
            USING ERRCODE = 'GD0E2';
    END IF;

    NEW.created_at := current_timestamp;
    RETURN NEW;
END
$$;

CREATE OR REPLACE TRIGGER trg_check_dod_timestamp BEFORE INSERT ON Dick_of_Day
    FOR EACH ROW EXECUTE FUNCTION check_dod_timestamp();

CREATE OR REPLACE FUNCTION forbid_dod_updates()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    RAISE EXCEPTION 'Updates of the Dick_of_Day table is forbidden!'
        USING ERRCODE = 'GD1E1';
END
$$;

CREATE OR REPLACE TRIGGER trg_forbid_dod_updates BEFORE UPDATE ON Dick_of_Day
    FOR EACH ROW EXECUTE FUNCTION forbid_dod_updates();
