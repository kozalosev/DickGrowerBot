use std::error::Error;
use std::fmt::Display;
use std::str::FromStr;
use anyhow::anyhow;
use reqwest::Url;
use teloxide::types::Me;
use crate::handlers::perks::HelpPussiesPerk;
use crate::handlers::utils::Incrementor;
use crate::help;

#[derive(Clone)]
pub struct AppConfig {
    pub features: FeatureToggles,
    pub top_limit: u16,
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
        let top_limit = get_env_value_or_default("TOP_LIMIT", 10);
        let chats_merging = get_env_value_or_default("CHATS_MERGING_ENABLED", false);
        let top_unlimited = get_env_value_or_default("TOP_UNLIMITED_ENABLED", false);
        let check_acceptor_length = get_env_value_or_default("PVP_CHECK_ACCEPTOR_LENGTH", false);
        Self {
            features: FeatureToggles {
                chats_merging,
                top_unlimited,
                pvp: BattlesFeatureToggles {
                    check_acceptor_length,
                }
            },
            top_limit,
        }
    }
}

impl DatabaseConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            url: get_env_mandatory_value("DATABASE_URL")?,
            max_connections: get_env_value_or_default("DATABASE_MAX_CONNECTIONS", 10)
        })
    }
}

pub fn build_context_for_help_messages(me: Me, incr: &Incrementor, competitor_bots: &[&str]) -> anyhow::Result<help::Context> {
    let other_bots = competitor_bots
        .iter()
        .map(|username| ensure_starts_with_at_sign(username.to_string()))
        .collect::<Vec<String>>()
        .join(", ");
    let incr_cfg = incr.get_config();

    Ok(help::Context {
        bot_name: me.username().to_owned(),
        grow_min: incr_cfg.growth_range_min().to_string(),
        grow_max: incr_cfg.growth_range_max().to_string(),
        other_bots,
        admin_username: ensure_starts_with_at_sign(get_env_mandatory_value("HELP_ADMIN_USERNAME")?),
        admin_channel_ru: ensure_starts_with_at_sign(get_env_mandatory_value("HELP_ADMIN_CHANNEL_RU")?),
        admin_channel_en: ensure_starts_with_at_sign(get_env_mandatory_value("HELP_ADMIN_CHANNEL_EN")?),
        git_repo: get_env_mandatory_value("HELP_GIT_REPO")?,
        help_pussies_percentage: incr.find_perk_config::<HelpPussiesPerk>()
            .map(|payout_ratio| payout_ratio * 100.0)
            .unwrap_or(0.0)
    })
}

pub(crate) fn get_env_mandatory_value<T, E>(key: &str) -> anyhow::Result<T>
where
    T: FromStr<Err = E>,
    E: Error + Send + Sync + 'static
{
    std::env::var(key)?
        .parse()
        .map_err(|e: E| anyhow!(e))
}

pub(crate) fn get_env_value_or_default<T, E>(key: &str, default: T) -> T
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
