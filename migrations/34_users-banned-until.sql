ALTER TABLE Users ADD COLUMN IF NOT EXISTS banned_until timestamptz;

COMMENT ON COLUMN Users.banned_until IS 'When the ban expires; NULL for a user who is not banned';

-- Nearly every row is NULL, so the index covers only the few banned ones.
CREATE INDEX IF NOT EXISTS idx_users_banned_until ON Users(banned_until)
    WHERE banned_until IS NOT NULL;
