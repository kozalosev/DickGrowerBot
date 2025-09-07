use std::error::Error;
use std::fmt::Display;
use std::str::FromStr;
use anyhow::anyhow;
use crate::domain::Ratio;

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

pub(super) fn get_mandatory_env_value<T, E>(key: &str) -> T
where
    T: FromStr<Err = E>,
    E: Error + Send + Sync + 'static
{
    get_env_mandatory_value(key).expect(&format!("{key} environment variable must be set!"))
}

pub(super) fn get_optional_env_value<T>(key: &str) -> T
where
    T: Default + FromStr + Display,
    <T as FromStr>::Err: Error + Send + Sync + 'static
{
    get_env_value_or_default(key, T::default())
}

pub(super) fn get_optional_env_ratio(key: &str) -> Option<Ratio> {
    let value = get_env_value_or_default(key, -1.0);
    Ratio::new(value)
        .inspect_err(|_| log::warn!("{key} is disabled due to the invalid value: {value}"))
        .ok()
}
