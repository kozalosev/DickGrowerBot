-- A journal of the group -> supergroup migrations we witnessed.
--
-- The supergroup's own chat_instance is deliberately not here: a service message never carries
-- one, so it could only be collected from some later callback. The old instance is already in the
-- Chats row and costs nothing to copy — it is NULL for a chat that had never been anchored.
--
-- A chat that isn't known by its old chat_id gets no record at all: there is no internal id to
-- attach it to, and no way to tell which instance-keyed row, if any, used to be that group.
CREATE TABLE IF NOT EXISTS Chat_Migrations (
    internal_id bigint PRIMARY KEY REFERENCES Chats(id),
    old_chat_id bigint NOT NULL,
    old_chat_instance varchar,
    new_chat_id bigint NOT NULL,
    migrated_at timestamp NOT NULL DEFAULT current_timestamp
);
