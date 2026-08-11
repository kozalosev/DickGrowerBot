use std::time::Duration;
use crate::config::env::*;

/// How long each cached value may be stale.
///
/// One lifetime per value, and they are not alike: what makes a chat's language worth keeping for
/// an hour says nothing about the bot's rights in it. Where the value is kept — this process or
/// Redis — is a separate question and does not belong here either.
///
/// A miss costs a query, or a question to Telegram, and nothing more. That is what sets these
/// apart from the settings of the services the bot talks to, and what makes the numbers a matter
/// of taste rather than of somebody else's rate limit.
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
    /// Whether the bot may delete other members' messages in a chat. A `my_chat_member` update
    /// writes it the moment it changes, so this only bounds how long a change missed while the bot
    /// was down goes unnoticed — which is why it is the shortest of the lot.
    pub bot_admin: Duration,
}

impl CachesConfig {
    pub fn from_env() -> Self {
        Self {
            chat_language: EnvDuration::seconds("CHAT_LANGUAGE_CACHE_TIME_SECONDS").or(3600).read(),
            chat_topics: EnvDuration::seconds("CHAT_TOPICS_CACHE_TIME_SECONDS").or(3600).read(),
            ban_list_refresh: EnvDuration::seconds("BAN_LIST_REFRESH_SECONDS").or(900).at_least(1).read(),
            bot_admin: EnvDuration::seconds("BOT_ADMIN_CACHE_TIME_SECONDS").or(3600).at_least(1).read(),
        }
    }
}
