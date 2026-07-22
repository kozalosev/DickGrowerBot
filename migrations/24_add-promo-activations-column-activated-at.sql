ALTER TABLE Promo_Code_Activations
    ADD COLUMN activated_at timestamptz,
    ALTER COLUMN uid SET NOT NULL,
    ALTER COLUMN code SET NOT NULL;
