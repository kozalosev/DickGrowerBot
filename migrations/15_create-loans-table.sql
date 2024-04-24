CREATE TABLE IF NOT EXISTS Loans (
    id serial PRIMARY KEY,
    uid bigint NOT NULL REFERENCES Users(uid),
    chat_id bigint NOT NULL REFERENCES Chats(id),
    debt int NOT NULL CHECK ( debt >= 0 ),
    created_at date NOT NULL DEFAULT current_date,
    repaid_at date
);

CREATE INDEX IF NOT EXISTS idx_loans_uid ON Loans(uid);

CREATE OR REPLACE FUNCTION set_date_if_debt_repaid()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    IF NEW.debt = 0 THEN
        NEW.repaid_at := current_date;
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE TRIGGER trg_set_date_if_debt_repaid BEFORE UPDATE ON Loans
    FOR EACH ROW EXECUTE FUNCTION set_date_if_debt_repaid();
