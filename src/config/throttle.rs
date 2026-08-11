use teloxide::adaptors::throttle::Limits;
use crate::config::env::env_value;
use crate::domain::primitives::RateLimit;

/// How fast the schedulers may talk to Telegram.
///
/// The numbers are Telegram's own limits, but we stay under them on purpose. The handlers answer
/// users through the plain `Bot` and are not counted here, so a scheduler that used the whole
/// allowance would push those answers into rate-limit errors. What is left over is their room.
#[derive(Clone, Copy)]
pub struct ThrottleConfig {
    pub messages_per_sec_overall: RateLimit,
    pub messages_per_sec_chat: RateLimit,
    /// Applies to private chats and to legacy groups.
    pub messages_per_min_chat: RateLimit,
    /// Applies to every chat whose id starts with -100, which is most of the ones the bot serves:
    /// teloxide picks this limit for supergroups as well as for channels.
    pub messages_per_min_channel_or_supergroup: RateLimit,
}

impl ThrottleConfig {
    pub fn from_env() -> Self {
        Self {
            messages_per_sec_overall: env_value!("THROTTLE_MESSAGES_PER_SEC_OVERALL": RateLimit, or = 20, at_least = 1),
            messages_per_sec_chat: env_value!("THROTTLE_MESSAGES_PER_SEC_CHAT": RateLimit, or = 1, at_least = 1),
            messages_per_min_chat: env_value!("THROTTLE_MESSAGES_PER_MIN_CHAT": RateLimit, or = 5, at_least = 1),
            messages_per_min_channel_or_supergroup: env_value!("THROTTLE_MESSAGES_PER_MIN_SUPERGROUP": RateLimit, or = 15, at_least = 1),
        }
    }
}

impl From<ThrottleConfig> for Limits {
    fn from(config: ThrottleConfig) -> Self {
        Self {
            messages_per_sec_overall: config.messages_per_sec_overall.value(),
            messages_per_sec_chat: config.messages_per_sec_chat.value(),
            messages_per_min_chat: config.messages_per_min_chat.value(),
            messages_per_min_channel_or_supergroup: config.messages_per_min_channel_or_supergroup.value(),
        }
    }
}
