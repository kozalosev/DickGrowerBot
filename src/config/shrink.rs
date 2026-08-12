use std::time::Duration;
use crate::domain::primitives::{AttemptsCount, DaysCount, Limit, Ratio};
use domain_types::literal;

/// Tuning for the daily job that shrinks dicks neglected for a while (issue #15), and for the
/// worker that delivers the summaries it owes (issue #154).
#[derive(Clone, Default)]
pub struct DailyShrinkConfig {
    pub ratio: Ratio,
    pub inactivity_days: DaysCount,
    pub ramp_up_days: DaysCount,
    /// How many chats one shrinking statement takes on. It bounds both the rows the statement locks
    /// and the ones a failure costs, so a `/grow` sent at midnight waits behind one batch rather
    /// than behind every stale dick in the database.
    pub batch_size: Limit,
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
    /// How long a summary rests after a failure that is worth another attempt.
    pub retry_delay: Duration,
    /// The longest a summary may rest between two attempts, however many have failed.
    pub max_retry_delay: Duration,
    /// How many attempts a summary gets before the row is marked `failed` and left alone.
    pub max_attempts: AttemptsCount,
    /// How old a summary may get before it stops being worth sending. Yesterday's list of shrinks
    /// is still news in a chat that reads once a day; last week's is noise.
    pub max_age: Duration,
    /// How long a finished row is kept before the cleaning process removes it. Zero keeps them for
    /// ever, which is what makes the queue's own history readable.
    pub retention: Duration,
}

impl DailyShrinkConfig {
    /// The daily shrink runs only when both knobs are meaningfully set: a positive ratio to lose
    /// and a positive grace period. Either being zero disables the feature — there's no separate flag.
    pub fn enabled(&self) -> bool {
        self.ratio > literal!(Ratio = 0.0) && self.inactivity_days.value() > 0
    }
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(5),
            batch_size: Limit::new(200),
            concurrency: Limit::new(16),
            lease: Duration::from_secs(300),
            retry_delay: Duration::from_secs(60),
            max_retry_delay: Duration::from_secs(3600),
            max_attempts: AttemptsCount::new(3),
            max_age: Duration::from_hours(48),
            retention: Duration::from_mins(4320),
        }
    }
}
