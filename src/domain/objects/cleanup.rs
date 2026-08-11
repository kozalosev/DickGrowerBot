use std::collections::BTreeMap;
use derive_more::Constructor;
use crate::domain::enums::MessageGroup;
use crate::domain::primitives::DelayMinutes;

/// How long each kind of the bot's messages lives in a chat that chose for itself.
///
/// A group that isn't here follows the bot's own configuration, and a group set to zero minutes is
/// kept for ever. The three states are all distinct: "keep these" and "you decide" lead to the same
/// message staying, but only the first of them survives the operator changing his mind.
///
/// An empty map is the default: a chat that never touched the setting has no `cleanup` key at all.
#[derive(Debug, Default, Clone, PartialEq, Eq, Constructor)]
pub struct ChatCleanupSettings(BTreeMap<MessageGroup, DelayMinutes>);

impl ChatCleanupSettings {
    /// How long the chat wants this group to live, or `None` when it left the choice to the bot.
    pub fn get(&self, group: MessageGroup) -> Option<DelayMinutes> {
        self.0.get(&group).copied()
    }

    /// Whether the chat follows the bot's configuration in everything.
    pub fn is_default(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_an_untouched_chat_chose_nothing() {
        let settings = ChatCleanupSettings::default();
        assert!(settings.is_default());
        assert_eq!(settings.get(MessageGroup::Notice), None);
    }

    #[test]
    fn test_a_choice_is_kept_apart_from_the_absence_of_one() {
        let settings = ChatCleanupSettings::new(BTreeMap::from([
            (MessageGroup::Notice, DelayMinutes::new(5)),
            (MessageGroup::Report, DelayMinutes::new(0)),
        ]));
        assert!(!settings.is_default());
        assert_eq!(settings.get(MessageGroup::Notice), Some(DelayMinutes::new(5)));
        assert_eq!(settings.get(MessageGroup::Report), Some(DelayMinutes::new(0)));
        assert_eq!(settings.get(MessageGroup::Event), None);
    }
}
