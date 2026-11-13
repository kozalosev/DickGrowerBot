use teloxide::Bot;
use teloxide::adaptors::throttle::{Limits, Settings, Throttle};
use crate::config::env::env_value;
use crate::domain::primitives::RateLimit;
use crate::metrics;

/// The bot everything talks to Telegram through, except the update listener.
///
/// The throttle counts requests in a worker task of its own, and a clone shares that worker, so
/// there must be exactly one of these per process.
pub type ThrottledBot = Throttle<Bot>;

/// How fast the bot may send messages to Telegram, answers and schedulers alike.
///
/// The per-minute limits are deliberately looser than Telegram's own: a chat held back by them
/// keeps its place in a queue every chat shares.
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
            messages_per_sec_overall: env_value!("THROTTLE_MESSAGES_PER_SEC_OVERALL": RateLimit, or = 30, at_least = 1),
            messages_per_sec_chat: env_value!("THROTTLE_MESSAGES_PER_SEC_CHAT": RateLimit, or = 1, at_least = 1),
            messages_per_min_chat: env_value!("THROTTLE_MESSAGES_PER_MIN_CHAT": RateLimit, or = 20, at_least = 1),
            messages_per_min_channel_or_supergroup: env_value!("THROTTLE_MESSAGES_PER_MIN_SUPERGROUP": RateLimit, or = 60, at_least = 1),
        }
    }

    pub fn wrap(self, bot: Bot) -> ThrottledBot {
        let settings = Settings::default()
            .limits(self.into())
            .on_queue_full(|pending| async move {
                metrics::TELEGRAM_THROTTLE_QUEUE_FULL.inc();
                tracing::warn!(pending, "the throttle queue is full, requests are waiting for their turn");
            });
        Throttle::spawn_with_settings(bot, settings)
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
