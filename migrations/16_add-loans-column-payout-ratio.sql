ALTER TABLE Loans ADD COLUMN IF NOT EXISTS payout_ratio real NOT NULL DEFAULT 0.1
    CHECK ( payout_ratio > 0.0 AND payout_ratio < 1.0 );
ALTER TABLE Loans ALTER COLUMN payout_ratio DROP DEFAULT;
