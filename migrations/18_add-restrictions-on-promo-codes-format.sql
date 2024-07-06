DO $$ BEGIN
    ALTER TABLE promo_codes ADD CONSTRAINT promo_code_format CHECK ( code ~ '^[a-zA-Z0-9_\-]{4,}$' );
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;
