use std::time::Duration;
use crate::config::env::*;
use crate::domain::primitives::{Limit, RateLimit};

/// How many requests a user, and a chat, may make in one window before the bot stops answering.
/// A limit of zero switches that half off.
#[derive(Clone, Copy, Default)]
pub struct RateLimitConfig {
    pub user: RateLimit,
    pub chat: RateLimit,
    pub window: Duration,
    /// How many requests past the limit are still told why they get nothing. Every one after them
    /// is dropped in silence, since answering a flood is how it spends Telegram's quota.
    pub warnings: Limit,
    pub commands: Commands,
}

impl RateLimitConfig {
    pub fn from_env() -> Self {
        Self {
            user: env_value!("RATE_LIMIT_USER_REQUESTS": RateLimit, or = 20),
            chat: env_value!("RATE_LIMIT_CHAT_REQUESTS": RateLimit, or = 60),
            window: env_duration!("RATE_LIMIT_WINDOW", or = mins(1), at_least = secs(1)),
            warnings: env_value!("RATE_LIMIT_WARNINGS": Limit, or = 3),
            commands: Commands {
                start: CommandSpamLimit {
                    requests: env_value!("SPAM_LIMIT_START_REQUESTS": RateLimit, or = 1),
                    window: env_duration!("SPAM_LIMIT_START_WINDOW", or = mins(10), at_least = secs(1)),
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Commands {
    /// How many bare `/start`s a chat is answered in a window; the rest are dropped in silence.
    pub start: CommandSpamLimit,
}

#[derive(Clone, Copy, Default)]
pub struct CommandSpamLimit {
    pub requests: RateLimit,
    pub window: Duration,
}
