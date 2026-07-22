-- Dick_of_Day.chat_id was converted from the raw Telegram id to the Chats surrogate id
-- back in migration 10, but (unlike Dicks in migration 14) never got the matching FK.
ALTER TABLE Dick_of_Day ADD FOREIGN KEY (chat_id) REFERENCES Chats(id);
