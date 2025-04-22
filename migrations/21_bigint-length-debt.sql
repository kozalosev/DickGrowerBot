BEGIN;
    ALTER TABLE Dicks        ALTER COLUMN length          TYPE BIGINT;
    ALTER TABLE Loans        ALTER COLUMN debt            TYPE BIGINT;
    ALTER TABLE Loans        ALTER COLUMN id              TYPE BIGINT;
    ALTER TABLE Battle_Stats ALTER COLUMN acquired_length TYPE BIGINT;
    ALTER TABLE Battle_Stats ALTER COLUMN lost_length     TYPE BIGINT;
    ALTER TABLE Imports      ALTER COLUMN original_length TYPE BIGINT;
COMMIT;
