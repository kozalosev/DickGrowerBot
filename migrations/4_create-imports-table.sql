CREATE TABLE IF NOT EXISTS Imports (
    chat_id bigint,
    uid bigint,
    original_length integer NOT NULL,
    imported_at timestamptz NOT NULL DEFAULT current_timestamp,

    PRIMARY KEY (chat_id, uid)
);
