use std::error::Error;
use std::fmt::Display;
use std::ops::RangeInclusive;
use std::str::FromStr;
use anyhow::anyhow;
use reqwest::Url;

#[derive(Clone)]
pub struct AppConfig {
    pub growth_range: RangeInclusive<i32>,
    pub dod_bonus_range: RangeInclusive<u32>,
}

#[derive(Clone)]
pub struct DatabaseConfig {
    pub url: Url,
    pub max_connections: u32
}

impl AppConfig {
    pub fn from_env() -> Self {
        let min = get_value_or_default("GROWTH_MIN", -5);
        let max = get_value_or_default("GROWTH_MAX", 10);
        let max_dod_bonus = get_value_or_default("GROWTH_DOD_BONUS_MAX", 5);
        Self {
            growth_range: min..=max,
            dod_bonus_range: 1..=max_dod_bonus
        }
    }
}

impl DatabaseConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            url: get_mandatory_value("DATABASE_URL")?,
            max_connections: get_value_or_default("DATABASE_MAX_CONNECTIONS", 5)
        })
    }
}

fn get_mandatory_value<T, E>(key: &str) -> anyhow::Result<T>
where
    T: FromStr<Err = E>,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)?
        .parse()
        .map_err(|e: E| anyhow!(e))
}

fn get_value_or_default<T, E>(key: &str, default: T) -> T
where
    T: FromStr<Err = E> + Display,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)
        .map_err(|e| {
            log::warn!("no value was found for an optional environment variable {key}, using the default value {default}");
            anyhow!(e)
        })
        .and_then(|v| v.parse()
            .map_err(|e: E| {
                log::warn!("invalid value of the {key} environment variable, using the default value {default}");
                anyhow!(e)
            }))
        .unwrap_or(default)
}
