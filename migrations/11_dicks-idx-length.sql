DROP INDEX IF EXISTS dicks_idx_chat_id;
CREATE INDEX IF NOT EXISTS dicks_idx_chat_id_length ON Dicks(chat_id, length DESC);
