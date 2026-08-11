use std::collections::BTreeMap;
use derive_more::Constructor;
use crate::domain::enums::MessageGroup;
use crate::domain::primitives::DelayMinutes;

/// What a chat decided about the bot's own messages in it: how long each kind lives, and whether
/// the inline ones are shrunk to a placeholder at all.
///
/// A group that isn't in the map follows the bot's own configuration, and a group set to zero
/// minutes is kept for ever. The three states are all distinct: "keep these" and "you decide" lead
/// to the same message staying, but only the first of them survives the operator changing his mind.
/// The inline flag is a tri-state for the same reason.
///
/// All-empty is the default: a chat that never touched the setting has no `cleanup` key at all.
#[derive(Debug, Default, Clone, PartialEq, Eq, Constructor)]
pub struct ChatCleanupSettings {
    delays: BTreeMap<MessageGroup, DelayMinutes>,
    /// Whether an inline message is replaced with the placeholder when its time is up. `None`
    /// leaves that to [`crate::config::SelfDestructionConfig::inline_groups`] — an inline message
    /// can't be deleted, only rewritten, so the bot's own list is the more careful default.
    inline: Option<bool>,
}

impl ChatCleanupSettings {
    /// How long the chat wants this group to live, or `None` when it left the choice to the bot.
    pub fn get(&self, group: MessageGroup) -> Option<DelayMinutes> {
        self.delays.get(&group).copied()
    }

    /// Whether the chat wants its inline messages shrunk, or `None` when it said nothing about them.
    pub fn compresses_inline(&self) -> Option<bool> {
        self.inline
    }

    /// Whether the chat follows the bot's configuration in everything.
    pub fn is_default(&self) -> bool {
        self.delays.is_empty() && self.inline.is_none()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn settings(choices: [(MessageGroup, u32); 2]) -> ChatCleanupSettings {
        let delays = choices.map(|(group, minutes)| (group, DelayMinutes::new(minutes)));
        ChatCleanupSettings::new(BTreeMap::from(delays), None)
    }

    #[test]
    fn test_an_untouched_chat_chose_nothing() {
        let settings = ChatCleanupSettings::default();
        assert!(settings.is_default());
        assert_eq!(settings.get(MessageGroup::Notice), None);
        assert_eq!(settings.compresses_inline(), None);
    }

    #[test]
    fn test_a_choice_is_kept_apart_from_the_absence_of_one() {
        let settings = settings([(MessageGroup::Notice, 5), (MessageGroup::Report, 0)]);
        assert!(!settings.is_default());
        assert_eq!(settings.get(MessageGroup::Notice), Some(DelayMinutes::new(5)));
        assert_eq!(settings.get(MessageGroup::Report), Some(DelayMinutes::new(0)));
        assert_eq!(settings.get(MessageGroup::Event), None);
    }

    /// The inline flag alone is a choice too — a chat that only touched it no longer follows the
    /// bot in everything, and must keep its "restore the defaults" button.
    #[test]
    fn test_the_inline_flag_counts_as_a_choice_of_its_own() {
        let settings = ChatCleanupSettings::new(BTreeMap::new(), Some(false));
        assert!(!settings.is_default());
        assert_eq!(settings.compresses_inline(), Some(false));
        assert_eq!(settings.get(MessageGroup::Notice), None);
    }
}
