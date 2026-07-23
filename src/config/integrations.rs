use std::env::VarError;
use reqwest::Url;
use crate::config::env::get_env_value_or_default;

const ENV_WEBHOOK_URL: &str = "WEBHOOK_URL";
const ENV_GRPC_ADDR_USER_SERVICE: &str = "GRPC_ADDR_USER_SERVICE";
const ENV_USER_CACHE_TIME_SECS: &str = "USER_CACHE_TIME_SECS";
const ENV_USER_SERVICE_TIMEOUT_SECS: &str = "USER_SERVICE_TIMEOUT_SECS";
const ENV_CHAT_LANGUAGE_CACHE_TIME_SECS: &str = "CHAT_LANGUAGE_CACHE_TIME_SECS";

const DEFAULT_USER_CACHE_TIME_SECS: u64 = 360;
const DEFAULT_USER_SERVICE_TIMEOUT_SECS: u64 = 5;
// We own this data (it lives in our own DB), so a rather aggressive TTL is fine.
const DEFAULT_CHAT_LANGUAGE_CACHE_TIME_SECS: u64 = 3600;

/// Configuration for connections to external services.
#[derive(Clone)]
pub struct IntegrationsConfig {
    pub webhook_url: Option<Url>,
    /// `Some` only when [`ENV_GRPC_ADDR_USER_SERVICE`] is configured; otherwise the whole
    /// user-service integration is disabled.
    pub user_service: Option<UserServiceConfig>,
    /// TTL of the per-chat language cache. Independent of the user-service — the chat-wide
    /// language works even when that integration is disabled.
    pub chat_language_cache_time_secs: u64,
}

#[derive(Clone)]
pub struct UserServiceConfig {
    pub address: String,
    pub cache_time_secs: u64,
    /// Per-request (and connection) timeout for gRPC calls, so a hanging service can't stall
    /// update processing — the call fails and language resolution falls back to Telegram's code.
    pub timeout_secs: u64,
}

impl IntegrationsConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            webhook_url: read_webhook_url()?,
            user_service: read_user_service_config(),
            chat_language_cache_time_secs: get_env_value_or_default(
                ENV_CHAT_LANGUAGE_CACHE_TIME_SECS, DEFAULT_CHAT_LANGUAGE_CACHE_TIME_SECS),
        })
    }
}

fn read_webhook_url() -> anyhow::Result<Option<Url>> {
    match std::env::var(ENV_WEBHOOK_URL) {
        Ok(url) if !url.is_empty() => Ok(Some(url.parse()?)),
        Ok(_) => Ok(None),
        Err(VarError::NotPresent) => Ok(None),
        Err(e) => Err(e)?,
    }
}

fn read_user_service_config() -> Option<UserServiceConfig> {
    std::env::var(ENV_GRPC_ADDR_USER_SERVICE)
        .ok()
        .filter(|addr| !addr.is_empty())
        .map(|address| UserServiceConfig {
            address,
            cache_time_secs: get_env_value_or_default(ENV_USER_CACHE_TIME_SECS, DEFAULT_USER_CACHE_TIME_SECS),
            timeout_secs: get_env_value_or_default(ENV_USER_SERVICE_TIMEOUT_SECS, DEFAULT_USER_SERVICE_TIMEOUT_SECS),
        })
}
