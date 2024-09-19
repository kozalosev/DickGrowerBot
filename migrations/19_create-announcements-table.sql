DO $$ BEGIN
    CREATE TYPE language_code AS ENUM (
        'en',
        'ru'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

CREATE TABLE IF NOT EXISTS Announcements (
    chat_id bigint REFERENCES Chats(id),
    language language_code,
    hash bytea NOT NULL,
    times_shown smallint NOT NULL CHECK ( times_shown >= 0 ),

    PRIMARY KEY (chat_id, language)
);

COMMENT ON TABLE  Announcements      IS 'A table to keep track on the amount of times some announcement was shown in each group chat';
COMMENT ON COLUMN Announcements.hash IS 'The SHA-256 hash of an announcement string set by application properties';
