ALTER TABLE Scheduled_Shrink_Broadcasts ADD COLUMN IF NOT EXISTS lang_code language_code;

COMMENT ON COLUMN Scheduled_Shrink_Broadcasts.lang_code IS 'The language the summary was rendered in, written the first time it is worked out; NULL means it has not been yet. Keeps a retry from resolving it again, and from answering the same chat in a different language than the attempt before.';
