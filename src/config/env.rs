use std::error::Error;
use std::fmt::Display;
use std::str::FromStr;
use std::time::Duration;
use anyhow::anyhow;
use crate::domain::primitives::chat::TelegramChatId;
use crate::domain::primitives::Ratio;

pub(super) fn get_env_mandatory_value<T, E>(key: &str) -> anyhow::Result<T>
where
    T: FromStr<Err = E>,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)?
        .parse()
        .map_err(|e: E| anyhow!(e))
}

pub fn get_env_value_or_default<T, E>(key: &str, default: T) -> T
where
    T: FromStr<Err = E> + Display,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)
        .map_err(|e| {
            tracing::warn!(key = %key, default = %default, "no value was found for an optional environment variable, using the default");
            anyhow!(e)
        })
        .and_then(|v| v.parse()
            .map_err(|e: E| {
                tracing::warn!(key = %key, default = %default, "invalid value of an environment variable, using the default");
                anyhow!(e)
            }))
        .unwrap_or(default)
}

pub(super) fn get_optional_env_value<T>(key: &str) -> T
where
    T: Default + FromStr + Display,
    <T as FromStr>::Err: Error + Send + Sync + 'static
{
    get_env_value_or_default(key, T::default())
}

/// Reads an optional environment variable holding a whole number of minutes and returns
/// it as a [`Duration`]. A missing or invalid value yields a zero duration.
pub(super) fn get_optional_env_minutes(key: &str) -> Duration {
    let minutes: u64 = get_optional_env_value(key);
    Duration::from_secs(minutes.saturating_mul(60))
}

pub(super) fn get_optional_env_ratio(key: &str) -> Option<Ratio> {
    let value = get_env_value_or_default(key, -1.0);
    Ratio::new(value)
        .inspect_err(|_| tracing::warn!(key = %key, value = %value, "the feature is disabled because of an invalid value"))
        .ok()
}

pub(super) fn get_chat_id(key: &str) -> Option<TelegramChatId> {
    std::env::var(key)
        .ok()
        .filter(|id| !id.is_empty())
        .and_then(|id| id.parse::<i64>()
             .inspect_err(|e| tracing::warn!(key = %key, error = %e, "chat_id is not a number"))
             .ok())
        .map(TelegramChatId::new)
}
