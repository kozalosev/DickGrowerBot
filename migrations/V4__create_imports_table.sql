CREATE TABLE IF NOT EXISTS Imports (
    chat_id bigint PRIMARY KEY,
    imported_at timestamptz NOT NULL DEFAULT current_timestamp
);
