CREATE TABLE IF NOT EXISTS Loans (
    id serial PRIMARY KEY,
    uid bigint NOT NULL REFERENCES Users(uid),
    chat_id bigint NOT NULL REFERENCES Chats(id),
    left_to_pay int NOT NULL CHECK ( left_to_pay >= 0 ),
    created_at date NOT NULL DEFAULT current_date,
    repaid_at date
);

CREATE INDEX IF NOT EXISTS idx_loans_uid ON Loans(uid);
