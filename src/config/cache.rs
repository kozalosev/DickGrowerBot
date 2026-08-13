use crate::config::env::*;

/// Where the values the bot keeps briefly are stored.
///
/// The default follows `REDIS_HOST`: with a server to talk to the bot shares them, and without one
/// it keeps them in this process, which is all a single instance ever needed. [`CacheMode::Disabled`]
/// is never fallen into — an operator asks for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum_macros::Display, strum_macros::EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
pub enum CacheMode {
    /// Shared by every instance of the bot.
    Redis,
    /// Kept in this process alone.
    Local,
    /// Nothing is kept, so every lookup does the work itself.
    Disabled,
}

/// Where to keep the cached values, and how to reach the server when they are shared.
pub struct CacheConfig {
    pub mode: CacheMode,
    /// `None` when `REDIS_HOST` is unset.
    pub url: Option<String>,
}

impl CacheConfig {
    pub fn from_env() -> Self {
        let url = redis_url();
        let mode = get_optional_env_string("CACHE_MODE")
            .and_then(|value| value.parse()
                .inspect_err(|e: &strum::ParseError| tracing::warn!(key = "CACHE_MODE", error = %e,
                    "invalid value of an environment variable, choosing the mode by REDIS_HOST instead"))
                .ok())
            .unwrap_or(if url.is_some() { CacheMode::Redis } else { CacheMode::Local });
        Self { mode, url }
    }

    /// A configuration pointing at a server, for the tests that start one. Production always goes
    /// through [`CacheConfig::from_env`].
    #[cfg(test)]
    pub fn redis(url: String) -> Self {
        Self { mode: CacheMode::Redis, url: Some(url) }
    }
}

fn redis_url() -> Option<String> {
    let host = get_optional_env_string("REDIS_HOST")?;
    let port: u16 = get_env_value_or_default("REDIS_PORT", 6379);
    let password = get_optional_env_string("REDIS_PASSWORD").unwrap_or_default();
    Some(format!("redis://:{password}@{host}:{port}/"))
}
