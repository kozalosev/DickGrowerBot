ALTER TABLE Dicks DROP CONSTRAINT IF EXISTS dicks_pkey;
ALTER TABLE Dicks ADD PRIMARY KEY (chat_id, uid);
