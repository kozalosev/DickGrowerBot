use std::time::Duration;

/// Lifetime category of a bot message, used to decide when (if ever) it self-destructs.
///
/// * `Notice` = canned, always-the-same messages (help, privacy, errors, statuses);
/// * `Report` = generated read-outs (leaderboard, stats);
/// * `Event` = permanent records (growths, DoDs, fights);
/// * `Application` = interactive requests (loans, battles).
///
/// In the proof-of-concept only [`MessageGroup::Notice`] and [`MessageGroup::Report`]
/// are ever scheduled for deletion; `Event` and `Application` are always permanent.
#[derive(Clone, Copy)]
pub enum MessageGroup {
    Notice,
    Report,
    // Defined for the full taxonomy but never scheduled in the PoC (permanent); they gain
    // call sites in the DB-backed pass (#127).
    #[allow(dead_code)]
    Event,
    #[allow(dead_code)]
    Application,
}

/// Per-group self-destruction delays. A zero delay means messages of that group are
/// permanent. The default (all-zero) disables the feature entirely, so it ships dark.
#[derive(Clone, Copy, Default)]
pub struct SelfDestructionConfig {
    pub notice: Duration,
    pub report: Duration,
}

impl SelfDestructionConfig {
    /// The configured delay for a group, or `None` if the group is permanent (zero delay
    /// or a group not schedulable in the PoC).
    pub fn delay_for(&self, group: MessageGroup) -> Option<Duration> {
        let delay = match group {
            MessageGroup::Notice => self.notice,
            MessageGroup::Report => self.report,
            // Events and applications are always permanent in the PoC (see #127).
            MessageGroup::Event | MessageGroup::Application => Duration::ZERO,
        };
        (!delay.is_zero()).then_some(delay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_delays_are_permanent() {
        let config = SelfDestructionConfig::default();
        assert_eq!(config.delay_for(MessageGroup::Notice), None);
        assert_eq!(config.delay_for(MessageGroup::Report), None);
        assert_eq!(config.delay_for(MessageGroup::Event), None);
        assert_eq!(config.delay_for(MessageGroup::Application), None);
    }

    #[test]
    fn non_zero_delays_are_returned() {
        let config = SelfDestructionConfig {
            notice: Duration::from_secs(120),
            report: Duration::from_secs(300),
        };
        assert_eq!(config.delay_for(MessageGroup::Notice), Some(Duration::from_secs(120)));
        assert_eq!(config.delay_for(MessageGroup::Report), Some(Duration::from_secs(300)));
        // Never schedulable in the PoC regardless of config.
        assert_eq!(config.delay_for(MessageGroup::Event), None);
        assert_eq!(config.delay_for(MessageGroup::Application), None);
    }
}
