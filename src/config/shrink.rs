use crate::domain::primitives::{DaysCount, Ratio};
use domain_types::literal;

/// Tuning for the daily job that shrinks dicks neglected for a while (issue #15).
#[derive(Clone, Default)]
pub struct DailyShrinkConfig {
    pub ratio: Ratio,
    pub inactivity_days: DaysCount,
    pub ramp_up_days: DaysCount,
}

impl DailyShrinkConfig {
    /// The daily shrink runs only when both knobs are meaningfully set: a positive ratio to lose
    /// and a positive grace period. Either being zero disables the feature — there's no separate flag.
    pub fn enabled(&self) -> bool {
        self.ratio > literal!(Ratio = 0.0) && self.inactivity_days.value() > 0
    }
}
