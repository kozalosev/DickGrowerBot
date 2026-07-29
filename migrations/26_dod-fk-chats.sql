-- Dick_of_Day.chat_id was converted from the raw Telegram id to the Chats surrogate id
-- back in migration 10, but (unlike Dicks in migration 14) never got the matching FK.
--
-- Without it, a chat merge could delete the losing Chats row and leave this table behind: Dicks
-- followed the row out through its own foreign key, while these rows kept pointing at a surrogate
-- id that no longer names anything. They can't be repaired — the id they hold is not a Telegram
-- one, and what it stood for is recorded nowhere — so they are dropped. The constraint below is
-- what stops them from ever accumulating again.
DELETE FROM Dick_of_Day dod
WHERE NOT EXISTS (SELECT 1 FROM Chats c WHERE c.id = dod.chat_id);

ALTER TABLE Dick_of_Day ADD FOREIGN KEY (chat_id) REFERENCES Chats(id);
