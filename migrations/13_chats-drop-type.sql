ALTER TABLE Chats DROP CONSTRAINT IF EXISTS chats_type_chat_id_key;
ALTER TABLE Chats DROP CONSTRAINT IF EXISTS chats_type_chat_instance_key;

ALTER TABLE Chats DROP CONSTRAINT IF EXISTS chats_chat_id_key;
ALTER TABLE Chats DROP CONSTRAINT IF EXISTS chats_chat_instance_key;
ALTER TABLE Chats ADD UNIQUE (chat_id);
ALTER TABLE Chats ADD UNIQUE (chat_instance);

ALTER TABLE Chats DROP CONSTRAINT IF EXISTS ck_chat_id;
ALTER TABLE Chats ADD CONSTRAINT ck_chat_id CHECK ( chat_id IS NOT NULL OR chat_instance IS NOT NULL );

ALTER TABLE Chats DROP COLUMN IF EXISTS type;
