use std::time::Duration;
use crate::config::env::{get_env_value_or_default, EnvDuration};

const ENV_REDIS_HOST: &str = "REDIS_HOST";
const ENV_REDIS_PORT: &str = "REDIS_PORT";
const ENV_REDIS_PASSWORD: &str = "REDIS_PASSWORD";
const ENV_REDIS_CACHE_TTL_SECS: &str = "REDIS_CACHE_TTL_SECS";

const DEFAULT_REDIS_PORT: u16 = 6379;
const DEFAULT_REDIS_CACHE_TTL_SECS: u64 = 21600;

/// Where to find Redis and how long the values kept there live.
///
/// `REDIS_HOST` is the switch: without it the cache is disabled and every caller falls back to
/// working without one, the same way `GRPC_ADDR_USER_SERVICE` gates the user-service.
pub struct RedisConfig {
    pub url: String,
    pub cache_ttl: Duration,
}

impl RedisConfig {
    pub fn from_env() -> Option<Self> {
        let host: String = std::env::var(ENV_REDIS_HOST).ok()
            .filter(|host| !host.is_empty())?;
        let port: u16 = get_env_value_or_default(ENV_REDIS_PORT, DEFAULT_REDIS_PORT);
        let password = std::env::var(ENV_REDIS_PASSWORD).unwrap_or_default();

        Some(Self {
            url: format!("redis://:{password}@{host}:{port}/"),
            cache_ttl: EnvDuration::seconds(ENV_REDIS_CACHE_TTL_SECS)
                .or(DEFAULT_REDIS_CACHE_TTL_SECS)
                .at_least(1)
                .read(),
        })
    }
}
