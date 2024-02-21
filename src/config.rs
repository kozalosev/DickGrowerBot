use std::error::Error;
use std::fmt::Display;
use std::ops::RangeInclusive;
use std::str::FromStr;
use anyhow::anyhow;
use reqwest::Url;
use teloxide::types::Me;
use crate::help;

#[derive(Clone)]
pub struct AppConfig {
    pub features: FeatureToggles,
    pub growth_range: RangeInclusive<i32>,
    pub grow_shrink_ratio: f32,
    pub dod_bonus_range: RangeInclusive<u32>,
    pub newcomers_grace_days: u32,
    pub top_limit: u32,
}

#[derive(Clone)]
pub struct DatabaseConfig {
    pub url: Url,
    pub max_connections: u32
}

#[derive(Clone, Copy)]
pub struct FeatureToggles {
    pub chats_merging: bool,
    pub top_unlimited: bool,
    pub pvp: BattlesFeatureToggles,
}

#[cfg(test)]
impl Default for FeatureToggles {
    fn default() -> Self {
        Self {
            chats_merging: true,
            top_unlimited: true,
            pvp: Default::default(),
        }
    }
}

#[derive(Copy, Clone, Default)]
pub struct BattlesFeatureToggles {
    pub check_acceptor_length: bool,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let min = get_value_or_default("GROWTH_MIN", -5);
        let max = get_value_or_default("GROWTH_MAX", 10);
        let grow_shrink_ratio = get_value_or_default("GROW_SHRINK_RATIO", 0.5);
        let max_dod_bonus = get_value_or_default("GROWTH_DOD_BONUS_MAX", 5);
        let newcomers_grace_days = get_value_or_default("NEWCOMERS_GRACE_DAYS", 7);
        let top_limit = get_value_or_default("TOP_LIMIT", 10);
        let chats_merging = get_value_or_default("CHATS_MERGING_ENABLED", false);
        let top_unlimited = get_value_or_default("TOP_UNLIMITED_ENABLED", false);
        let check_acceptor_length = get_value_or_default("PVP_CHECK_ACCEPTOR_LENGTH", false);
        Self {
            features: FeatureToggles {
                chats_merging,
                top_unlimited,
                pvp: BattlesFeatureToggles {
                    check_acceptor_length,
                }
            },
            growth_range: min..=max,
            grow_shrink_ratio,
            dod_bonus_range: 1..=max_dod_bonus,
            newcomers_grace_days,
            top_limit,
        }
    }
}

impl DatabaseConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            url: get_mandatory_value("DATABASE_URL")?,
            max_connections: get_value_or_default("DATABASE_MAX_CONNECTIONS", 10)
        })
    }
}

pub fn build_context_for_help_messages(me: Me, app_config: &AppConfig, competitor_bots: &[&str]) -> anyhow::Result<help::Context> {
    let other_bots = competitor_bots
        .iter()
        .map(|username| ensure_starts_with_at_sign(username.to_string()))
        .collect::<Vec<String>>()
        .join(", ");

    Ok(help::Context {
        bot_name: me.username().to_owned(),
        grow_min: app_config.growth_range.clone().min().ok_or(anyhow!("growth_range must have min"))?.to_string(),
        grow_max: app_config.growth_range.clone().max().ok_or(anyhow!("growth_range must have max"))?.to_string(),
        other_bots,
        admin_username: ensure_starts_with_at_sign(get_mandatory_value("HELP_ADMIN_USERNAME")?),
        admin_channel: ensure_starts_with_at_sign(get_mandatory_value("HELP_ADMIN_CHANNEL")?),
        git_repo: get_mandatory_value("HELP_GIT_REPO")?,
    })
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

fn ensure_starts_with_at_sign(s: String) -> String {
    if s.starts_with('@') {
        s
    } else {
        format!("@{s}")
    }
}

#[cfg(test)]
mod test {
    use super::ensure_starts_with_at_sign;

    #[test]
    fn test_ensure_starts_with_at_sign() {
        let result = "@test";
        assert_eq!(ensure_starts_with_at_sign("test".to_owned()), result);
        assert_eq!(ensure_starts_with_at_sign("@test".to_owned()), result);
    }
}
