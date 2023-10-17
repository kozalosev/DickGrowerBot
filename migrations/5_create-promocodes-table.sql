CREATE TABLE IF NOT EXISTS Promo_Codes (
    code varchar(16) PRIMARY KEY,
    bonus_length integer NOT NULL,
    since date NOT NULL DEFAULT current_date,
    until date,
    capacity integer NOT NULL CHECK ( capacity >= 0 )
);

CREATE TABLE IF NOT EXISTS Promo_Code_Activations (
    uid bigint REFERENCES Users(uid),
    code varchar(16) REFERENCES Promo_Codes(code),
    affected_chats integer NOT NULL,

    PRIMARY KEY (uid, code)
);
