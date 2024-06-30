CREATE TABLE IF NOT EXISTS Battle_Stats (
    uid     bigint REFERENCES Users(uid) ON DELETE CASCADE,
    chat_id bigint REFERENCES Chats(id) ON DELETE CASCADE,

    battles_total int NOT NULL DEFAULT 0 CHECK ( battles_total >= 0 ),
    battles_won   int NOT NULL DEFAULT 0 CHECK ( battles_won >= 0 ),

    win_streak_current smallint NOT NULL DEFAULT 0 CHECK ( win_streak_current >= 0 ),
    win_streak_max     smallint NOT NULL DEFAULT 0 CHECK ( win_streak_max >= win_streak_current ),

    PRIMARY KEY (uid, chat_id)
);

CREATE OR REPLACE FUNCTION update_win_streak_max_if_needed()
    RETURNS TRIGGER
    LANGUAGE PLPGSQL
AS $$
BEGIN
    IF NEW.win_streak_current > NEW.win_streak_max THEN
        NEW.win_streak_max = NEW.win_streak_current;
    END IF;
    RETURN NEW;
END
$$;

CREATE OR REPLACE TRIGGER trg_update_win_streak_max_if_needed BEFORE INSERT OR UPDATE ON Battle_Stats
    FOR EACH ROW EXECUTE FUNCTION update_win_streak_max_if_needed();
