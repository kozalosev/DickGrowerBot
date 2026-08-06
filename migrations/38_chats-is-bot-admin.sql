-- Whether the bot may delete other members' messages here, which is what it needs to remove the
-- command behind a self-destructing answer. NULL means Telegram was never asked; the answer is
-- filled in on the first deletion and re-checked whenever one is refused.
ALTER TABLE Chats ADD COLUMN IF NOT EXISTS is_bot_admin boolean;
