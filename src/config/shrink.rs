use crate::domain::primitives::{DaysCount, Ratio};

/// Tuning for the daily job that shrinks dicks neglected for a while (issue #15).
#[derive(Clone, Default)]
pub struct StaleDicksShrinkingConfig {
    pub ratio: Ratio,
    pub grace_period_days: DaysCount,
    pub history_window_days: DaysCount,
}

impl StaleDicksShrinkingConfig {
    /// The daily shrink runs only when both knobs are meaningfully set: a positive ratio to lose
    /// and a positive grace period. Either being zero disables the feature — there's no separate flag.
    pub fn enabled(&self) -> bool {
        self.ratio > Ratio::literal(0.0) && self.grace_period_days.value() > 0
    }
}
