-- A legacy (basic) group's `inline_message_id` doesn't encode the chat: it decodes to an unrelated
-- positive number, usually the inline sender's user id, which used to be trusted as a chat_id.
-- No chat the bot stores can have a positive one — it only plays in groups, supergroups and
-- channels, all of them negative — so the sign alone marks the damaged rows.
--
-- Dropping the invented chat_id leaves such a row keyed by its chat_instance alone, which is the
-- shape merge_chats folds into the group's own row on the next anchoring tap. Until now that tap
-- failed every time, both rows having a chat_id, so the group could never be anchored at all.
--
-- The chat_instance is what makes a row repairable: one poisoned without it identifies no chat any
-- more, and nulling its chat_id would leave nothing to find it by. There are none, and the
-- constraint below refuses to apply if that ever stops being true.
UPDATE Chats SET chat_id = NULL WHERE chat_id > 0 AND chat_instance IS NOT NULL;

ALTER TABLE Chats DROP CONSTRAINT IF EXISTS ck_chat_id_not_positive;
ALTER TABLE Chats ADD CONSTRAINT ck_chat_id_not_positive CHECK (chat_id IS NULL OR chat_id < 0);
