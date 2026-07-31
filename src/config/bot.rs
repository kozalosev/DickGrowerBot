use std::time::Duration;
use teloxide::Bot;

const ENV_CONNECT_TIMEOUT_SECS: &str = "BOT_HTTP_CONNECT_TIMEOUT_SECS";
const ENV_TIMEOUT_SECS: &str = "BOT_HTTP_TIMEOUT_SECS";

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
            connect_timeout: read_optional_secs(ENV_CONNECT_TIMEOUT_SECS),
            timeout: read_optional_secs(ENV_TIMEOUT_SECS),
        }
    }

    /// Reads the config from the environment and builds the bot with the HTTP client
    /// configured accordingly.
    pub fn build_bot() -> anyhow::Result<Bot> {
        let cfg = Self::from_env();
        if cfg.connect_timeout.is_none() && cfg.timeout.is_none() {
            // Nothing to configure => keep teloxide's stock client (it also honors TELOXIDE_PROXY).
            return Ok(Bot::from_env());
        }
        // `default_reqwest_settings()` returns teloxide's own reqwest builder (5s connect, 17s
        // total, tcp_nodelay); we override only the timeouts that were explicitly configured.
        let mut builder = teloxide::net::default_reqwest_settings();
        if let Some(connect_timeout) = cfg.connect_timeout {
            builder = builder.connect_timeout(connect_timeout);
        }
        if let Some(timeout) = cfg.timeout {
            builder = builder.timeout(timeout);
        }
        Ok(Bot::from_env_with_client(builder.build()?))
    }
}

/// Reads an optional environment variable holding a whole number of seconds and returns it as a
/// [`Duration`]. A missing, empty, or unparseable value yields `None`.
fn read_optional_secs(key: &str) -> Option<Duration> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u64>()
            .inspect_err(|e| log::warn!("invalid value of the {key} environment variable: {e}"))
            .ok())
        .map(Duration::from_secs)
}
