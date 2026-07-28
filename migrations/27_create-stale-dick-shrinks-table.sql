CREATE TABLE IF NOT EXISTS Stale_Dick_Shrinks (
    chat_id bigint NOT NULL REFERENCES Chats(id),
    uid bigint NOT NULL REFERENCES Users(uid),
    lost_length bigint NOT NULL,
    created_at date DEFAULT current_date,

    PRIMARY KEY (chat_id, uid, created_at)
);

CREATE OR REPLACE FUNCTION forbid_stale_dick_shrinks_updates()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    RAISE EXCEPTION 'Updates of the Stale_Dick_Shrinks table is forbidden!'
        USING ERRCODE = 'GD2E1';
END
$$;

CREATE OR REPLACE TRIGGER trg_forbid_stale_dick_shrinks_updates BEFORE UPDATE ON Stale_Dick_Shrinks
    FOR EACH ROW EXECUTE FUNCTION forbid_stale_dick_shrinks_updates();
