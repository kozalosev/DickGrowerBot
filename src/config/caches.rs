use std::time::Duration;
use crate::config::env::*;

/// How long our own data may be stale in memory.
///
/// Every one of these reads from our own database, so a miss costs a query and nothing else. That
/// is what sets them apart from the settings of the services the bot talks to, and what makes the
/// numbers a matter of taste rather than of somebody else's rate limit.
#[derive(Clone, Default)]
pub struct CachesConfig {
    /// The chat-wide language. The command's own writes refresh it, so this only bounds how long
    /// another instance's change goes unnoticed.
    pub chat_language: Duration,
    /// The allowed topics of a forum, on the same terms.
    pub chat_topics: Duration,
    /// How often the ban list is re-read. Never zero: it is an interval as much as a lifetime,
    /// and a zero one is a busy loop.
    pub ban_list_refresh: Duration,
}

impl CachesConfig {
    pub fn from_env() -> Self {
        Self {
            chat_language: EnvDuration::seconds("CHAT_LANGUAGE_CACHE_TIME_SECS").or(3600).read(),
            chat_topics: EnvDuration::seconds("CHAT_TOPICS_CACHE_TIME_SECS").or(3600).read(),
            ban_list_refresh: EnvDuration::seconds("BAN_LIST_REFRESH_SECS").or(900).at_least(1).read(),
        }
    }
}
