DROP TRIGGER IF EXISTS trg_check_dod_timestamp ON Dick_of_Day;
DROP TRIGGER IF EXISTS trg_forbid_dod_updates ON Dick_of_Day;
DROP FUNCTION IF EXISTS check_dod_timestamp();
DROP FUNCTION IF EXISTS forbid_dod_updates();
