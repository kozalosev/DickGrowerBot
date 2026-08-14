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
    /// Which of the bot's messages a chat has it clean up, on the same terms.
    pub chat_cleanup: Duration,
    /// How often the ban list is re-read. Never zero: it is an interval as much as a lifetime,
    /// and a zero one is a busy loop.
    pub ban_list_refresh: Duration,
    /// Whether the bot may delete other members' messages in a chat. A `my_chat_member` update
    /// writes it the moment it changes, so this only bounds how long a change missed while the bot
    /// was down goes unnoticed — which is why it is the shortest of the lot.
    pub bot_admin: Duration,
    /// How long a battle stays locked when the handler holding it never gets to let go.
    ///
    /// The guard frees it as the handler ends, so this only bounds a killed process — but it must
    /// stay above the longest a handler can take. A lock that runs out under a working handler lets
    /// the next answer through, and the same attack is resolved twice. Generous is cheap: the
    /// restart after the death that leaves a lock behind takes longer anyway.
    pub pvp_lock: Duration,
}

impl CachesConfig {
    pub fn from_env() -> Self {
        Self {
            chat_language: env_duration!("CHAT_LANGUAGE_CACHE_TIME", or = hours(1)),
            chat_topics: env_duration!("CHAT_TOPICS_CACHE_TIME", or = hours(1)),
            chat_cleanup: env_duration!("CHAT_CLEANUP_CACHE_TIME", or = hours(1)),
            ban_list_refresh: env_duration!("BAN_LIST_REFRESH", or = mins(15), at_least = secs(1)),
            bot_admin: env_duration!("BOT_ADMIN_CACHE_TIME", or = hours(1), at_least = secs(1)),
            pvp_lock: env_duration!("PVP_LOCK_TIME", or = mins(3), at_least = secs(1)),
        }
    }
}
