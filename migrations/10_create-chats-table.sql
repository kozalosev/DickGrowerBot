DO $$ BEGIN
    CREATE TYPE chat_id_type AS ENUM (
        'id',
        'inst'
        );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

CREATE TABLE IF NOT EXISTS Chats(
    id bigserial PRIMARY KEY,
    type chat_id_type NOT NULL,
    chat_id bigint,
    chat_instance varchar,

    UNIQUE (type, chat_id),
    UNIQUE (type, chat_instance),
    CONSTRAINT ck_chat_id CHECK (
        type = 'id' AND chat_id IS NOT NULL
            OR
        type = 'inst' AND chat_instance IS NOT NULL)
);

DROP TRIGGER IF EXISTS trg_forbid_dod_updates ON Dick_of_Day;

DO $$
DECLARE
    c_count integer := 0;
BEGIN
    SELECT count(*) INTO c_count FROM Chats;
    IF c_count = 0 THEN
        INSERT INTO Chats (type, chat_id) SELECT DISTINCT 'id'::chat_id_type, chat_id FROM Dicks;
        UPDATE Dicks d SET bonus_attempts = (bonus_attempts + 1), chat_id = id FROM Chats c WHERE c.chat_id = d.chat_id;
        UPDATE Dick_of_Day dod SET chat_id = id FROM Chats c WHERE c.chat_id = dod.chat_id;
    END IF;
END $$;

CREATE OR REPLACE TRIGGER trg_forbid_dod_updates BEFORE UPDATE ON Dick_of_Day
    FOR EACH ROW EXECUTE FUNCTION forbid_dod_updates();
