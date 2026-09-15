-- The window includes both of its ends, so a code valid for a single day has since = until.
ALTER TABLE Promo_Codes DROP CONSTRAINT IF EXISTS promo_code_window;
ALTER TABLE Promo_Codes ADD CONSTRAINT promo_code_window CHECK (until IS NULL OR since <= until);
