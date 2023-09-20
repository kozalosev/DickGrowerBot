CREATE TABLE IF NOT EXISTS Users (
    uid bigint PRIMARY KEY,
    name varchar(128) NOT NULL
);

INSERT INTO Users(uid, name) SELECT uid, '' FROM Dicks;

ALTER TABLE Dicks ADD FOREIGN KEY (uid) REFERENCES Users(uid) ON DELETE CASCADE;
