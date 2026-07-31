-- Chats the bot can't post to: kicked, blocked, muted, or simply gone. The daily shrink learns this
-- from a failed broadcast and stops trying every night. The flag is cleared as soon as a command is
-- processed in the chat again — that's a proof the bot is back.
ALTER TABLE Chats ADD COLUMN IF NOT EXISTS is_unreachable boolean NOT NULL DEFAULT false;
