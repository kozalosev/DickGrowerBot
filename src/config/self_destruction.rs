use std::str::FromStr;
use std::time::Duration;
use crate::domain::primitives::{AttemptsCount, Limit};

/// The latest a message may be scheduled for. Telegram refuses to let a bot delete anything older
/// than 48 hours, and the hour of headroom is for everything that happens between the moment a
/// message falls due and the moment the request goes out: the poll interval, the lease, the
/// warning's grace period and the waits between failed attempts. Scheduled at the full 48, a
/// message would be past the limit before it was ever tried.
pub const MAX_DELAY: Duration = Duration::from_secs(47 * 60 * 60);

/// Lifetime category of a bot message, used to decide when (if ever) it self-destructs.
///
/// * `Notice` = canned, always-the-same messages (help, privacy, errors, statuses);
/// * `Report` = generated read-outs (leaderboard, stats);
/// * `Event` = permanent records (growths, DoDs, fights);
/// * `Application` = interactive requests (loans, battles).
///
/// The lowercase spelling is shared by the `message_group` enum of the database, the label of
/// [`crate::metrics::SELF_DESTRUCTION`] and the log field, so the three can't drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, sqlx::Type,
         strum_macros::Display, strum_macros::EnumString, strum_macros::EnumIter)]
#[strum(serialize_all = "lowercase")]
#[sqlx(type_name = "message_group", rename_all = "lowercase")]
pub enum MessageGroup {
    Notice,
    Report,
    Event,
    Application,
}

/// What the bot does with the command behind a self-destructing answer. The default leaves it
/// alone: a bot that is already an administrator must not start erasing its members' messages
/// just because it was updated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, strum_macros::Display, strum_macros::EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum DeletionMode {
    /// Nothing self-destructs, whatever the delays say.
    Disabled,
    /// The answer always goes; the command goes too when the bot is allowed to remove it.
    Enabled,
    /// The answer goes only together with its command — all or nothing.
    OnlyWithCommand,
    /// The answer goes, the command is never touched and Telegram is never asked about the rights.
    #[default]
    WithoutCommand,
}

/// The groups whose inline messages are replaced with a placeholder, as a set that stays `Copy`.
/// Parsed from a comma-separated list of group names; an empty list disables the inline half.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct InlineGroups(u8);

impl InlineGroups {
    fn bit(group: MessageGroup) -> u8 {
        1 << (group as u8)
    }

    pub fn contains(&self, group: MessageGroup) -> bool {
        self.0 & Self::bit(group) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }
}

impl FromStr for InlineGroups {
    type Err = strum::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .try_fold(0, |acc, name| MessageGroup::from_str(&name.to_lowercase())
                .map(|group| acc | Self::bit(group)))
            .map(Self)
    }
}

impl std::fmt::Display for InlineGroups {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use strum::IntoEnumIterator;
        let names = MessageGroup::iter()
            .filter(|group| self.contains(*group))
            .map(|group| group.to_string())
            .collect::<Vec<_>>();
        f.write_str(&names.join(","))
    }
}

/// Per-group self-destruction delays and tuning. A zero group delay means messages of
/// that group are permanent. The default (all-zero) disables the feature entirely, so it
/// ships dark.
#[derive(Clone, Copy, Default)]
pub struct SelfDestructionConfig {
    pub notice: Duration,
    pub report: Duration,
    pub event: Duration,
    pub application: Duration,
    /// Visible characters an average reader gets through per minute; the base delay of a
    /// long message is stretched to at least its estimated reading time. A value of 0
    /// disables the reading-time adjustment.
    pub reading_speed_cpm: u64,
    /// Grace period during which the message is replaced with a "will be deleted" warning
    /// before it is actually removed. Zero deletes the message without any warning.
    pub warning: Duration,
    pub mode: DeletionMode,
    /// How often the worker looks for the messages whose time has come.
    pub poll_interval: Duration,
    /// How many messages one run of the worker takes on.
    pub batch_size: Limit,
    /// How long a claimed batch stays out of every other worker's reach.
    pub lease: Duration,
    pub inline_groups: InlineGroups,
    /// How long a message rests after a failure that is worth another attempt.
    pub retry_delay: Duration,
    /// The longest a message may rest between two attempts, however many have failed.
    pub max_retry_delay: Duration,
    /// How many attempts a message gets before the row is marked `failed` and left alone.
    pub max_attempts: AttemptsCount,
    /// How long a finished row (expired or failed) is kept before the cleaning process removes it.
    /// Zero keeps them for ever, which is what makes the queue's own history readable.
    pub retention: Duration,
}

impl SelfDestructionConfig {
    /// The configured delay for a group, or `None` if the group is permanent (a zero delay, or
    /// the whole feature switched off). Delays longer than [`MAX_DELAY`] are cut down to it.
    pub fn delay_for(&self, group: MessageGroup) -> Option<Duration> {
        if let DeletionMode::Disabled = self.mode {
            return None
        }
        let delay = match group {
            MessageGroup::Notice => self.notice,
            MessageGroup::Report => self.report,
            MessageGroup::Event => self.event,
            MessageGroup::Application => self.application,
        };
        (!delay.is_zero()).then(|| delay.min(MAX_DELAY))
    }

    /// Whether anything at all may self-destruct — used to decide if the worker is worth spawning.
    pub fn enabled(&self) -> bool {
        use strum::IntoEnumIterator;
        MessageGroup::iter().any(|group| self.delay_for(group).is_some())
    }

    /// Whether the command behind an answer is scheduled together with it.
    pub fn deletes_commands(&self) -> bool {
        matches!(self.mode, DeletionMode::Enabled | DeletionMode::OnlyWithCommand)
    }

    /// Whether an answer whose command can't be deleted is left alone as well.
    pub fn requires_command(&self) -> bool {
        matches!(self.mode, DeletionMode::OnlyWithCommand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(config: SelfDestructionConfig) -> SelfDestructionConfig {
        SelfDestructionConfig { mode: DeletionMode::Enabled, ..config }
    }

    #[test]
    fn zero_delays_are_permanent() {
        let config = enabled(SelfDestructionConfig::default());
        assert_eq!(config.delay_for(MessageGroup::Notice), None);
        assert_eq!(config.delay_for(MessageGroup::Report), None);
        assert_eq!(config.delay_for(MessageGroup::Event), None);
        assert_eq!(config.delay_for(MessageGroup::Application), None);
        assert!(!config.enabled());
    }

    #[test]
    fn non_zero_delays_are_returned() {
        let config = enabled(SelfDestructionConfig {
            notice: Duration::from_secs(120),
            report: Duration::from_secs(300),
            event: Duration::from_secs(3600),
            application: Duration::from_secs(1800),
            ..Default::default()
        });
        assert_eq!(config.delay_for(MessageGroup::Notice), Some(Duration::from_secs(120)));
        assert_eq!(config.delay_for(MessageGroup::Report), Some(Duration::from_secs(300)));
        assert_eq!(config.delay_for(MessageGroup::Event), Some(Duration::from_secs(3600)));
        assert_eq!(config.delay_for(MessageGroup::Application), Some(Duration::from_secs(1800)));
        assert!(config.enabled());
    }

    #[test]
    fn too_long_delays_are_cut_down() {
        let config = enabled(SelfDestructionConfig {
            application: Duration::from_secs(72 * 60 * 60),
            ..Default::default()
        });
        assert_eq!(config.delay_for(MessageGroup::Application), Some(MAX_DELAY));
    }

    #[test]
    fn the_disabled_mode_makes_everything_permanent() {
        let config = SelfDestructionConfig {
            notice: Duration::from_secs(120),
            mode: DeletionMode::Disabled,
            ..Default::default()
        };
        assert_eq!(config.delay_for(MessageGroup::Notice), None);
        assert!(!config.enabled());
    }

    #[test]
    fn the_mode_governs_the_command() {
        let mode = |mode| SelfDestructionConfig { mode, ..Default::default() };
        assert!(!mode(DeletionMode::WithoutCommand).deletes_commands());
        assert!(!mode(DeletionMode::WithoutCommand).requires_command());
        assert!(mode(DeletionMode::Enabled).deletes_commands());
        assert!(!mode(DeletionMode::Enabled).requires_command());
        assert!(mode(DeletionMode::OnlyWithCommand).deletes_commands());
        assert!(mode(DeletionMode::OnlyWithCommand).requires_command());
    }

    #[test]
    fn inline_groups_are_parsed_from_a_list() {
        let groups: InlineGroups = "notice, REPORT".parse().expect("couldn't parse the groups");
        assert!(groups.contains(MessageGroup::Notice));
        assert!(groups.contains(MessageGroup::Report));
        assert!(!groups.contains(MessageGroup::Event));
        assert!(!groups.is_empty());
        assert_eq!(groups.to_string(), "notice,report");
    }

    #[test]
    fn an_empty_list_of_inline_groups_is_empty() {
        let groups: InlineGroups = "".parse().expect("couldn't parse the groups");
        assert!(groups.is_empty());
        assert!(!groups.contains(MessageGroup::Notice));
    }

    #[test]
    fn an_unknown_inline_group_is_an_error() {
        assert!("notice,dickpic".parse::<InlineGroups>().is_err());
    }
}
