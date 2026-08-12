DO $$ BEGIN
    CREATE TYPE broadcast_state AS ENUM (
        'created',
        'sent',
        'unreachable',
        'expired',
        'failed'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

CREATE TABLE IF NOT EXISTS Scheduled_Shrink_Broadcasts (
    id bigserial PRIMARY KEY,
    chat_id bigint NOT NULL REFERENCES Chats(id),
    shrink_date date NOT NULL,
    fire_after timestamptz NOT NULL DEFAULT current_timestamp,
    state broadcast_state NOT NULL DEFAULT 'created',
    attempts int NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT current_timestamp,
    finished_at timestamptz
);

-- The two partial indexes are complements: the claim only ever reads unfinished rows and the
-- cleaner only ever reads finished ones, so each index leaves out exactly what the other is about.
CREATE INDEX IF NOT EXISTS Scheduled_Shrink_Broadcasts_fire_after_idx
    ON Scheduled_Shrink_Broadcasts (fire_after) WHERE finished_at IS NULL;
CREATE INDEX IF NOT EXISTS Scheduled_Shrink_Broadcasts_finished_at_idx
    ON Scheduled_Shrink_Broadcasts (finished_at) WHERE finished_at IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS Scheduled_Shrink_Broadcasts_chat_date_idx
    ON Scheduled_Shrink_Broadcasts (chat_id, shrink_date);

COMMENT ON TABLE  Scheduled_Shrink_Broadcasts             IS 'The daily shrink summaries a chat is owed, and what became of the ones that are done with; the cleaning process takes the latter away';
COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.chat_id     IS 'The internal id of the chat, so that a group migrated to a supergroup is addressed by its new Telegram id at send time';
COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.shrink_date IS 'The day whose shrinks the summary is about; the text is read from Stale_Dick_Shrinks by it';
COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.fire_after  IS 'The moment the row becomes a candidate; the worker polls, so it acts somewhat later';
COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.attempts    IS 'Failed attempts to send the summary; the row is given up on after a few of them';
COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.created_at  IS 'When the shrink that owes this summary was committed';
COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.finished_at IS 'When the row was sent, expired or given up on; NULL while it is still actionable';
