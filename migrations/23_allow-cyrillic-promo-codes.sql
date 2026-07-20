ALTER TABLE promo_codes DROP CONSTRAINT IF EXISTS promo_code_format;
ALTER TABLE promo_codes ADD CONSTRAINT promo_code_format
    CHECK ( code ~ '^[a-zA-Zа-яА-ЯёЁ0-9_\-]{4,}$' );
