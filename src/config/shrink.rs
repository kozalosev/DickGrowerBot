use std::time::Duration;
use chrono::{Datelike, NaiveDate};
use crate::domain::primitives::{AttemptsCount, DaysCount, Limit, Ratio};
use domain_types::literal;

/// Tuning for the daily job that shrinks dicks neglected for a while (issue #15), and for the
/// worker that delivers the summaries it owes (issue #154).
#[derive(Clone, Default)]
pub struct DailyShrinkConfig {
    pub ratio: Ratio,
    pub inactivity_days: DaysCount,
    pub ramp_up_days: DaysCount,
    /// Days between two runs.
    pub period_days: DaysCount,
    /// How many chats one shrinking statement takes on. It bounds both the rows the statement locks
    /// and the ones a failure costs, so a `/grow` sent at midnight waits behind one batch rather
    /// than behind every stale dick in the database.
    pub batch_size: Limit,
    /// How long the run rests between two batches. Zero runs them back to back, which is what it did
    /// before there was a knob for it. Nothing waits on this job, so the rest costs only a longer
    /// run — and buys back the connections the chats being answered at midnight are queueing for.
    pub batch_delay: Duration,
    /// How many leftover summaries one statement of the nightly cleanup finishes as expired. It
    /// bounds the rows that statement locks and the time it may spend inside `statement_timeout`,
    /// not how much gets cleared: the cleanup repeats until the queue is empty.
    pub expiry_batch_size: Limit,
    pub broadcast: BroadcastConfig,
}

/// Tuning for the worker that sends the queued shrink summaries.
#[derive(Clone)]
pub struct BroadcastConfig {
    /// How often the worker looks for the summaries whose time has come.
    pub poll_interval: Duration,
    /// How many summaries one run of the worker claims.
    pub batch_size: Limit,
    /// How many of them it sends at once. What one run gets through is this many messages per round
    /// trip to Telegram, so this is the knob for throughput and `batch_size` only bounds how much a
    /// run claims.
    pub concurrency: Limit,
    /// How long a claimed batch stays out of every other worker's reach.
    pub lease: Duration,
    /// The longest a single send may take before the worker gives up on it and moves on. A backstop
    /// against a hang that `BOT_HTTP_TIMEOUT`/`BOT_HTTP_CONNECT_TIMEOUT` don't cover — e.g. a stuck
    /// `Throttle` queue entry, whose wait happens before the HTTP client is even asked to send
    /// anything, so no request-level timeout ever sees it. Without this, one such hang freezes the
    /// whole worker: the tick that owns it never returns, so `ticker.tick()` is never awaited again.
    pub send_timeout: Duration,
    /// How long a summary rests after a failure that is worth another attempt.
    pub retry_delay: Duration,
    /// The longest a summary may rest between two attempts, however many have failed.
    pub max_retry_delay: Duration,
    /// How many attempts a summary gets before the row is marked `failed` and left alone.
    pub max_attempts: AttemptsCount,
    /// How long a finished row is kept before the cleaning process removes it. Zero keeps them for
    /// ever, which is what makes the queue's own history readable.
    pub retention: Duration,
    /// How many of a chat's players the language tally looks at, when the chat has chosen no
    /// language of its own. A tally does not become truer for having every last member in it, and
    /// the whole roll of a large chat is a list the user-service then has to look up in one go.
    pub language_sample: Limit,
}

impl DailyShrinkConfig {
    /// The daily shrink runs only when both knobs are meaningfully set: a positive ratio to lose
    /// and a positive grace period. Either being zero disables the feature — there's no separate flag.
    pub fn enabled(&self) -> bool {
        self.ratio > literal!(Ratio = 0.0) && !self.inactivity_days.is_zero()
    }

    /// Whether the shrink runs on `date`: its day number since 0001-01-01 is a multiple of the period.
    /// A zero period means daily.
    pub fn is_due(&self, date: NaiveDate) -> bool {
        i64::from(date.num_days_from_ce())
            .checked_rem_euclid(i64::from(self.period_days.value()))
            .is_none_or(|rest| rest == 0)
    }
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            batch_size: Limit::new(200),
            concurrency: Limit::new(16),
            lease: Duration::from_mins(5),
            send_timeout: Duration::from_secs(30),
            retry_delay: Duration::from_mins(1),
            max_retry_delay: Duration::from_hours(1),
            max_attempts: AttemptsCount::new(3),
            retention: Duration::from_hours(72),
            language_sample: Limit::new(100),
        }
    }
}

#[cfg(test)]
mod test {
    use chrono::{Datelike, NaiveDate};
    use crate::domain::primitives::DaysCount;
    use super::DailyShrinkConfig;

    #[test]
    fn a_daily_period_is_due_every_day() {
        let config = config(1);
        assert!(config.is_due(date(2026, 10, 15)));
        assert!(config.is_due(date(2026, 10, 16)));
    }

    #[test]
    fn a_zero_period_is_read_as_daily() {
        assert!(config(0).is_due(date(2026, 10, 15)));
    }

    #[test]
    fn a_weekly_period_falls_on_one_weekday() {
        let config = config(7);
        let due: Vec<_> = (1..=28)
            .map(|day| date(2026, 10, day))
            .filter(|&d| config.is_due(d))
            .collect();
        assert_eq!(due.len(), 4);
        let weekday = due[0].weekday();
        assert!(due.iter().all(|d| d.weekday() == weekday));
    }

    #[test]
    fn a_longer_period_skips_the_days_between() {
        let config = config(30);
        let due = (0..90)
            .map(|offset| date(2026, 1, 1) + chrono::Days::new(offset))
            .filter(|&d| config.is_due(d))
            .count();
        assert_eq!(due, 3);
    }

    fn config(period: u32) -> DailyShrinkConfig {
        DailyShrinkConfig { period_days: DaysCount::new(period), ..Default::default() }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("invalid date")
    }
}
