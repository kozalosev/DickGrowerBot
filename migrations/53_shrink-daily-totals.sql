-- How much the shrink took away each day, so the total doesn't need a scan of the whole log.
-- Created empty: the first refresh fills it, outside the migrations that block the startup.
CREATE MATERIALIZED VIEW IF NOT EXISTS Shrink_Daily_Totals AS
    SELECT created_at AS shrink_date, sum(lost_length)::bigint AS lost_length, count(*) AS victims
    FROM Stale_Dick_Shrinks
    GROUP BY created_at
    WITH NO DATA;

-- REFRESH ... CONCURRENTLY requires a unique index.
CREATE UNIQUE INDEX IF NOT EXISTS shrink_daily_totals_idx_date ON Shrink_Daily_Totals (shrink_date);
