use reqwest::Url;
use std::time::Duration;
use crate::config::env::*;

/// Configuration for connections to external services.
#[derive(Clone)]
pub struct IntegrationsConfig {
    pub webhook_url: Option<Url>,
    /// `Some` only when `GRPC_ADDR_USER_SERVICE` is configured; otherwise the whole user-service
    /// integration is disabled.
    pub user_service: Option<UserServiceConfig>,
    /// TTL of the per-chat language cache. Independent of the user-service — the chat-wide
    /// language works even when that integration is disabled. We own this data, it lives in our
    /// own database, so a rather aggressive TTL is fine.
    pub chat_language_cache_ttl: Duration,
    /// TTL of the per-chat allowed-topics cache. The setting's own writes refresh it, so this
    /// only bounds how long another instance's change goes unnoticed.
    pub chat_topics_cache_ttl: Duration,
}

#[derive(Clone)]
pub struct UserServiceConfig {
    pub address: String,
    /// How long a fetched user is kept, and how often the sweeper runs. Never zero: it is also an
    /// interval, and a zero one is a busy loop.
    pub cache_ttl: Duration,
    /// Per-request (and connection) timeout for gRPC calls, so a hanging service can't stall
    /// update processing — the call fails and language resolution falls back to Telegram's code.
    pub timeout: Duration,
}

impl IntegrationsConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            // An address that is set but unusable is worth stopping for; one that is absent means
            // long polling, which is how the bot runs in development.
            webhook_url: get_optional_env_string("WEBHOOK_URL")
                .map(|url| url.parse())
                .transpose()?,
            user_service: UserServiceConfig::from_env(),
            chat_language_cache_ttl: EnvDuration::seconds("CHAT_LANGUAGE_CACHE_TIME_SECS").or(3600).read(),
            chat_topics_cache_ttl: EnvDuration::seconds("CHAT_TOPICS_CACHE_TIME_SECS").or(3600).read(),
        })
    }
}

impl UserServiceConfig {
    fn from_env() -> Option<Self> {
        let address = get_optional_env_string("GRPC_ADDR_USER_SERVICE")?;
        Some(Self {
            address,
            cache_ttl: EnvDuration::seconds("USER_CACHE_TIME_SECS").or(360).at_least(1).read(),
            timeout: EnvDuration::seconds("USER_SERVICE_TIMEOUT_SECS").or(5).read(),
        })
    }
}
