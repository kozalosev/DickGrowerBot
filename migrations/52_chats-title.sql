-- The title of a group, as Telegram last reported it; NULL until the bot has seen one.
ALTER TABLE Chats ADD COLUMN IF NOT EXISTS title varchar(255);
