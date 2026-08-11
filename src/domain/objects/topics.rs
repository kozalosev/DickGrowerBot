use std::collections::BTreeSet;
use derive_more::Constructor;
use crate::domain::primitives::chat::TopicId;

/// The forum topics a chat lets the bot work in.
///
/// Ids only, with no names: the Bot API can't be asked for a topic's name, and a set of `#42`
/// labels tells a reader nothing a plain count doesn't. So the picker speaks about the topic it
/// was opened in, and a topic is allowed or forbidden from inside it.
///
/// An empty set means the chat is unrestricted, which is the default: a chat that never touched
/// the setting has no `topics` key at all.
#[derive(Debug, Default, Clone, PartialEq, Eq, Constructor)]
pub struct AllowedTopics(BTreeSet<TopicId>);

impl AllowedTopics {
    pub fn is_unrestricted(&self) -> bool {
        self.0.is_empty()
    }

    pub fn allows(&self, topic: TopicId) -> bool {
        self.is_unrestricted() || self.0.contains(&topic)
    }

    /// How many topics the bot is confined to. Zero means it isn't.
    pub fn count(&self) -> usize {
        self.0.len()
    }

    /// The topic to post into when the bot speaks on its own (a broadcast) rather than replying —
    /// a reply already goes where the message it answers is. `None` leaves the choice to Telegram,
    /// which means General.
    ///
    /// The lowest id, because a jsonb object doesn't preserve the order its keys were added in:
    /// the order the topics were chosen in can't be recovered, so a stable rule beats an arbitrary
    /// one that could change between reads.
    pub fn primary(&self) -> Option<TopicId> {
        self.0.iter().next().copied()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn topics(ids: &[u32]) -> AllowedTopics {
        AllowedTopics::new(ids.iter().map(|id| TopicId::new(*id)).collect())
    }

    #[test]
    fn test_unrestricted_allows_everything() {
        let allowed = AllowedTopics::default();
        assert!(allowed.is_unrestricted());
        assert!(allowed.allows(TopicId::GENERAL));
        assert!(allowed.allows(TopicId::new(42)));
        assert_eq!(allowed.count(), 0);
        assert_eq!(allowed.primary(), None);
    }

    #[test]
    fn test_restricted_allows_only_the_listed_topics() {
        let allowed = topics(&[42]);
        assert!(!allowed.is_unrestricted());
        assert!(allowed.allows(TopicId::new(42)));
        assert!(!allowed.allows(TopicId::GENERAL));
        assert!(!allowed.allows(TopicId::new(43)));
        assert_eq!(allowed.count(), 1);
    }

    #[test]
    fn test_primary_is_the_lowest_id() {
        let allowed = topics(&[42, 7, 100]);
        assert_eq!(allowed.primary(), Some(TopicId::new(7)));
        assert_eq!(allowed.count(), 3);
    }
}
