-- An empty chat_instance is indistinguishable from a real one to every query that only checks for
-- NULL — is_anchored would call such a chat anchored and let the commands through, and a migration
-- would report it as having come across whole. Telegram never sends one, so the only way to get
-- there is a hand-written UPDATE; this turns that into an error instead of a silent corruption.
UPDATE Chats SET chat_instance = NULL WHERE chat_instance = '';

ALTER TABLE Chats DROP CONSTRAINT IF EXISTS ck_chat_instance_not_empty;
ALTER TABLE Chats ADD CONSTRAINT ck_chat_instance_not_empty CHECK (chat_instance <> '');
