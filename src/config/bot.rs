use std::time::Duration;
use crate::config::env::get_optional_env_seconds;
use teloxide::Bot;
use crate::telegram_observer::TelegramObserver;

/// Configuration of the bot's HTTP client (the one talking to the Telegram Bot API).
///
/// The timeouts exist so that a stalled request — e.g. when the ТСПУ (DPI) equipment lets the
/// connection hang and crawl instead of resetting it — fails after a bounded time instead of
/// blocking update processing indefinitely.
///
/// Both fields are optional; a `None` leaves teloxide's own default for that knob in place. When
/// *both* are unset, no custom client is built at all and the stock [`Bot::from_env`] is used.
#[derive(Clone, Copy)]
pub struct BotConfig {
    pub connect_timeout: Option<Duration>,
    /// Total per-request timeout.
    pub timeout: Option<Duration>,
}

impl BotConfig {
    fn from_env() -> Self {
        Self {
            connect_timeout: get_optional_env_seconds("BOT_HTTP_CONNECT_TIMEOUT_SECONDS"),
            timeout: get_optional_env_seconds("BOT_HTTP_TIMEOUT_SECONDS"),
        }
    }

    /// Reads the config from the environment and builds the bot with the HTTP client
    /// configured accordingly.
    pub fn build_bot() -> anyhow::Result<Bot> {
        let cfg = Self::from_env();
        let bot = if cfg.connect_timeout.is_none() && cfg.timeout.is_none() {
            // Nothing to configure => keep teloxide's stock client (it also honors TELOXIDE_PROXY).
            Bot::from_env()
        } else {
            // `default_reqwest_settings()` returns teloxide's own reqwest builder (5s connect, 17s
            // total, tcp_nodelay); we override only the timeouts that were explicitly configured.
            let mut builder = teloxide::net::default_reqwest_settings();
            if let Some(connect_timeout) = cfg.connect_timeout {
                builder = builder.connect_timeout(connect_timeout);
            }
            if let Some(timeout) = cfg.timeout {
                builder = builder.timeout(timeout);
            }
            Bot::from_env_with_client(builder.build()?)
        };
        Ok(bot.set_observer(TelegramObserver))
    }
}
