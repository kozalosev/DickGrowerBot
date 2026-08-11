use std::str::FromStr;
use std::time::Duration;
use crate::domain::objects::ChatCleanupSettings;
use crate::domain::primitives::{AttemptsCount, DelayMinutes, Limit};

pub use crate::domain::enums::MessageGroup;

/// The latest a message may be scheduled for. Telegram refuses to let a bot delete anything older
/// than 48 hours, and the hour of headroom is for everything that happens between the moment a
/// message falls due and the moment the request goes out: the poll interval, the lease, the
/// warning's grace period and the waits between failed attempts. Scheduled at the full 48, a
/// message would be past the limit before it was ever tried.
pub const MAX_DELAY: Duration = Duration::from_secs(47 * 60 * 60);

/// What the bot does with the command behind a self-destructing answer. The default takes both
/// away: a chat that asked for the answers to go wants no half-dialogues left behind, and the
/// command is only touched where the bot may delete messages anyway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, strum_macros::Display, strum_macros::EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum DeletionMode {
    /// Nothing self-destructs, whatever the delays say.
    Disabled,
    /// The answer always goes; the command goes too when the bot is allowed to remove it.
    #[default]
    Enabled,
    /// The answer goes only together with its command — all or nothing.
    OnlyWithCommand,
    /// The answer goes, the command is never touched and Telegram is never asked about the rights.
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

/// The delays `/cleanup` offers a chat to choose from, in minutes, sorted and without repetitions.
///
/// Only real delays are held here: "keep for ever" and "as the bot does" are added by the picker
/// itself, so no list can leave a chat with a choice it can't take back. A zero and anything past
/// [`MAX_DELAY`] are dropped when the variable is read — the first says nothing the picker doesn't
/// already offer, and the second is a delay the bot could never honour.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DelayOptions(Vec<DelayMinutes>);

/// A minute for the impatient, an hour for the tolerant, and three steps in between. Unset means
/// this rather than nothing, because a bot that offers no delays offers no choice worth the
/// command; a list emptied on purpose still says "keep things only".
impl Default for DelayOptions {
    fn default() -> Self {
        Self([1, 5, 15, 60, 180].map(DelayMinutes::new).into())
    }
}

impl DelayOptions {
    pub fn iter(&self) -> impl Iterator<Item = DelayMinutes> + '_ {
        self.0.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromStr for DelayOptions {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut minutes = s.split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.parse::<u32>())
            .collect::<Result<Vec<_>, _>>()?;
        minutes.retain(|value| {
            let acceptable = *value > 0 && u64::from(*value) * 60 <= MAX_DELAY.as_secs();
            if !acceptable {
                tracing::warn!(minutes = %value, "a suggested delay is dropped: it is either zero or past the limit");
            }
            acceptable
        });
        minutes.sort_unstable();
        minutes.dedup();
        Ok(Self(minutes.into_iter().map(DelayMinutes::new).collect()))
    }
}

impl std::fmt::Display for DelayOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let values = self.iter()
            .map(|minutes| minutes.to_string())
            .collect::<Vec<_>>();
        f.write_str(&values.join(","))
    }
}

/// Per-group self-destruction delays and tuning. A zero group delay means messages of
/// that group are permanent. The default (all-zero) disables the feature entirely, so it
/// ships dark.
#[derive(Clone, Default)]
pub struct SelfDestructionConfig {
    pub notice: Duration,
    pub report: Duration,
    pub event: Duration,
    pub application: Duration,
    /// The delays a chat may choose from with `/cleanup`. An empty list leaves it nothing to pick,
    /// so its administrators can only keep messages that the bot would otherwise take away.
    pub delay_options: DelayOptions,
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
    /// How many of them it acts on at once. What one run gets through is this many messages per
    /// round trip to Telegram, so this is the knob for throughput and `batch_size` only bounds how
    /// much a run claims.
    pub concurrency: Limit,
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
        self.capped(self.base_delay(group))
    }

    /// The same for a chat that chose for itself: the delay it picked, zero minutes meaning it
    /// wants the group kept for ever. A chat that chose nothing gets [`Self::delay_for`].
    ///
    /// A value that is no longer among [`Self::delay_options`] still applies: the list is a
    /// suggestion for the next press, not a rule about what may already be stored.
    pub fn delay_for_chat(&self, group: MessageGroup, settings: &ChatCleanupSettings) -> Option<Duration> {
        if let DeletionMode::Disabled = self.mode {
            return None
        }
        match settings.get(group) {
            Some(minutes) => self.capped(Duration::from_secs(u64::from(minutes.value()) * 60)),
            None => self.delay_for(group),
        }
    }

    /// Whether anything at all may self-destruct — used to decide if the worker is worth spawning.
    ///
    /// A chat may pick a delay for a group the bot itself keeps for ever, so the offered list
    /// counts as much as the four delays do. With nothing offered, a chat can only keep messages.
    pub fn enabled(&self) -> bool {
        use strum::IntoEnumIterator;
        MessageGroup::iter().any(|group| self.delay_for(group).is_some())
            || (self.configurable() && !self.delay_options.is_empty())
    }

    /// Whether a chat may be offered the setting at all. `DISABLED` is the operator's kill switch:
    /// there is nothing to configure under it.
    pub fn configurable(&self) -> bool {
        !matches!(self.mode, DeletionMode::Disabled)
    }

    fn base_delay(&self, group: MessageGroup) -> Duration {
        match group {
            MessageGroup::Notice => self.notice,
            MessageGroup::Report => self.report,
            MessageGroup::Event => self.event,
            MessageGroup::Application => self.application,
        }
    }

    /// A delay as it is actually used: cut down to [`MAX_DELAY`], and `None` when it means
    /// "permanent".
    fn capped(&self, delay: Duration) -> Option<Duration> {
        (!delay.is_zero()).then(|| delay.min(MAX_DELAY))
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

    /// A configuration with nothing offered to the chats either, so that `enabled()` answers about
    /// the four delays alone.
    fn offering_nothing(config: SelfDestructionConfig) -> SelfDestructionConfig {
        SelfDestructionConfig { delay_options: DelayOptions(Vec::new()), ..enabled(config) }
    }

    #[test]
    fn zero_delays_are_permanent() {
        let config = enabled(SelfDestructionConfig::default());
        assert_eq!(config.delay_for(MessageGroup::Notice), None);
        assert_eq!(config.delay_for(MessageGroup::Report), None);
        assert_eq!(config.delay_for(MessageGroup::Event), None);
        assert_eq!(config.delay_for(MessageGroup::Application), None);
    }

    /// The worker is still worth running with every delay at zero, because a chat can pick one of
    /// the offered delays for itself. With nothing offered there is nothing left to wait for.
    #[test]
    fn a_bot_that_offers_a_choice_is_enabled_by_it() {
        assert!(enabled(SelfDestructionConfig::default()).enabled());
        assert!(!offering_nothing(SelfDestructionConfig::default()).enabled());
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

    fn chose(group: MessageGroup, minutes: u32) -> ChatCleanupSettings {
        ChatCleanupSettings::new([(group, DelayMinutes::new(minutes))].into())
    }

    #[test]
    fn a_chat_that_chose_nothing_follows_the_bot() {
        let config = enabled(SelfDestructionConfig {
            notice: Duration::from_secs(120),
            ..Default::default()
        });
        let settings = ChatCleanupSettings::default();
        assert_eq!(config.delay_for_chat(MessageGroup::Notice, &settings), Some(Duration::from_secs(120)));
        assert_eq!(config.delay_for_chat(MessageGroup::Report, &settings), None);
    }

    #[test]
    fn a_chosen_delay_wins_over_the_bots_own() {
        let config = enabled(SelfDestructionConfig {
            notice: Duration::from_secs(120),
            ..Default::default()
        });
        let settings = chose(MessageGroup::Notice, 5);
        assert_eq!(config.delay_for_chat(MessageGroup::Notice, &settings), Some(Duration::from_secs(300)));
    }

    #[test]
    fn a_group_kept_for_ever_is_permanent_in_that_chat() {
        let config = enabled(SelfDestructionConfig {
            notice: Duration::from_secs(120),
            ..Default::default()
        });
        let settings = chose(MessageGroup::Notice, 0);
        assert_eq!(config.delay_for_chat(MessageGroup::Notice, &settings), None);
    }

    /// The direction the whole feature exists for: a chat cleaning up what the bot keeps.
    #[test]
    fn a_delay_can_be_chosen_for_a_group_the_bot_keeps() {
        let config = enabled(SelfDestructionConfig::default());
        let settings = chose(MessageGroup::Event, 5);
        assert_eq!(config.delay_for_chat(MessageGroup::Event, &settings), Some(Duration::from_secs(300)));
    }

    #[test]
    fn a_chat_cant_choose_anything_in_a_disabled_bot() {
        let config = SelfDestructionConfig {
            mode: DeletionMode::Disabled,
            ..Default::default()
        };
        let settings = chose(MessageGroup::Event, 5);
        assert_eq!(config.delay_for_chat(MessageGroup::Event, &settings), None);
        assert!(!config.configurable());
        assert!(!config.enabled());
    }

    /// Nothing stops a stored value from outliving the list it was picked from, so the cap has to
    /// hold on the way out as well.
    #[test]
    fn a_chosen_delay_is_cut_down_too() {
        let config = enabled(SelfDestructionConfig::default());
        let settings = chose(MessageGroup::Event, 72 * 60);
        assert_eq!(config.delay_for_chat(MessageGroup::Event, &settings), Some(MAX_DELAY));
    }

    #[test]
    fn delay_options_are_parsed_sorted_and_deduped() {
        let options: DelayOptions = "60, 5,15,5".parse().expect("couldn't parse the options");
        assert_eq!(options.iter().collect::<Vec<_>>(),
                   [5, 15, 60].map(DelayMinutes::new));
        assert_eq!(options.to_string(), "5,15,60");
        assert!(!options.is_empty());
    }

    /// A zero says nothing the picker doesn't offer anyway, and a delay past the cap could never
    /// be honoured — both are dropped rather than turning the whole variable down.
    #[test]
    fn unusable_delay_options_are_dropped() {
        let options: DelayOptions = "0,5,,4000".parse().expect("couldn't parse the options");
        assert_eq!(options.iter().collect::<Vec<_>>(), [DelayMinutes::new(5)]);
    }

    #[test]
    fn an_empty_list_of_delay_options_leaves_nothing_to_choose() {
        let options: DelayOptions = "".parse().expect("couldn't parse the options");
        assert!(options.is_empty());
        assert_eq!(options.to_string(), "");
    }

    #[test]
    fn a_delay_option_that_is_not_a_number_is_an_error() {
        assert!("5,soon".parse::<DelayOptions>().is_err());
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
