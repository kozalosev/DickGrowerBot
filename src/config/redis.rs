use crate::config::env::*;

/// Where to find Redis and how long the values kept there live.
///
/// `REDIS_HOST` is the switch: without it the cache is disabled and every caller falls back to working without one.
pub struct RedisConfig {
    pub url: String,
}

impl RedisConfig {
    pub fn from_env() -> Option<Self> {
        let host = get_optional_env_string("REDIS_HOST")?;
        let port: u16 = get_env_value_or_default("REDIS_PORT", 6379);
        let password = get_optional_env_string("REDIS_PASSWORD").unwrap_or_default();

        Some(Self {
            url: format!("redis://:{password}@{host}:{port}/"),
        })
    }
}
