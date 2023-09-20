CREATE TABLE IF NOT EXISTS Dicks (
    uid bigint,
    chat_id bigint,
    length integer NOT NULL DEFAULT 0,
    updated_at timestamptz NOT NULL DEFAULT current_timestamp,

    PRIMARY KEY (uid, chat_id)
);

CREATE OR REPLACE FUNCTION update_dicks_timestamp()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    NEW.updated_at := current_timestamp;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE TRIGGER trg_update_dicks_timestamp BEFORE INSERT OR UPDATE ON Dicks
    FOR EACH ROW EXECUTE FUNCTION update_dicks_timestamp();
