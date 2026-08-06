DO $$ BEGIN
    CREATE TYPE message_group AS ENUM (
        'notice',
        'report',
        'event',
        'application'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

DO $$ BEGIN
    CREATE TYPE message_kind AS ENUM (
        'reply',
        'command',
        'inline'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

DO $$ BEGIN
    CREATE TYPE deletion_state AS ENUM (
        'created',
        'warned',
        'removed',
        'removed_before',
        'expired',
        'failed'
    );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;

CREATE TABLE IF NOT EXISTS Scheduled_Message_Deletions (
    id bigserial PRIMARY KEY,
    chat_id bigint,
    message_id int,
    inline_message_id text,
    message_kind message_kind NOT NULL,
    message_group message_group NOT NULL,
    lang_code text NOT NULL,
    fire_after timestamptz NOT NULL,
    state deletion_state NOT NULL DEFAULT 'created',
    attempts int NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT current_timestamp,
    finished_at timestamptz,

    CHECK ( (chat_id IS NOT NULL AND message_id IS NOT NULL) <> (inline_message_id IS NOT NULL) )
);

CREATE INDEX IF NOT EXISTS Scheduled_Message_Deletions_fire_after_idx
    ON Scheduled_Message_Deletions (fire_after);
CREATE INDEX IF NOT EXISTS Scheduled_Message_Deletions_finished_at_idx
    ON Scheduled_Message_Deletions (finished_at) WHERE finished_at IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS Scheduled_Message_Deletions_chat_message_idx
    ON Scheduled_Message_Deletions (chat_id, message_id)
    WHERE chat_id IS NOT NULL AND message_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS Scheduled_Message_Deletions_inline_message_idx
    ON Scheduled_Message_Deletions (inline_message_id) WHERE inline_message_id IS NOT NULL;

COMMENT ON TABLE  Scheduled_Message_Deletions                   IS 'Messages waiting for their self-destruction, and what became of the ones that are done with; the cleaning process takes the latter away';
COMMENT ON COLUMN Scheduled_Message_Deletions.chat_id           IS 'The Telegram id of the chat, NULL for an inline message';
COMMENT ON COLUMN Scheduled_Message_Deletions.inline_message_id IS 'Set for inline messages only: they cannot be deleted, so they are replaced with a placeholder';
COMMENT ON COLUMN Scheduled_Message_Deletions.lang_code         IS 'The language of the warning and of the placeholder';
COMMENT ON COLUMN Scheduled_Message_Deletions.fire_after        IS 'The moment the row becomes a candidate; the worker polls, so it acts somewhat later';
COMMENT ON COLUMN Scheduled_Message_Deletions.attempts          IS 'Failed attempts to act on the message; the row is given up on after a few of them';
COMMENT ON COLUMN Scheduled_Message_Deletions.created_at        IS 'When the message was scheduled, which is when it was sent — a message older than 48 hours can no longer be deleted by a bot';
COMMENT ON COLUMN Scheduled_Message_Deletions.finished_at       IS 'When the row was expired or given up on; NULL while it is still actionable';
