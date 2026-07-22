use std::time::Duration;
use reqwest::Url;
use crate::config::env::*;
use crate::config::toggles::*;
use crate::config::announcements::*;
use crate::config::self_destruction::*;
use crate::domain::primitives::{Bet, DaysCount, Limit, Ratio};
use crate::domain::primitives::SupportedLanguage::{EN, RU, IT, FA, ZH};

#[derive(Clone)]
#[cfg_attr(test, derive(Default))]
pub struct AppConfig {
    pub features: FeatureToggles,
    pub top_limit: Limit,
    pub inactivity_days: DaysCount,
    pub loan_payout_ratio: Ratio,
    pub dod_rich_exclusion_ratio: Option<Ratio>,
    pub pvp_default_bet: Bet,
    pub announcements: AnnouncementsConfig,
    pub self_destruction: SelfDestructionConfig,
    pub command_toggles: CachedEnvToggles,
}

#[derive(Clone)]
pub struct DatabaseConfig {
    pub url: Url,
    pub max_connections: u32
}

impl AppConfig {
    pub fn from_env() -> Self {
        let top_limit = get_env_value_or_default("TOP_LIMIT", Limit::literal(10));
        let inactivity_days = get_env_value_or_default("INACTIVITY_DAYS", DaysCount::new(7));
        let loan_payout_ratio = get_env_value_or_default("LOAN_PAYOUT_COEF", Ratio::literal(0.0));
        let dod_selection_mode = get_optional_env_value("DOD_SELECTION_MODE");
        let dod_rich_exclusion_ratio = get_optional_env_ratio("DOD_RICH_EXCLUSION_RATIO");
        let chats_merging = get_env_value_or_default("CHATS_MERGING_ENABLED", false);
        let top_unlimited = get_env_value_or_default("TOP_UNLIMITED_ENABLED", false);
        let multiple_loans = get_env_value_or_default("MULTIPLE_LOANS_ENABLED", false);
        let pvp_default_bet = get_env_value_or_default("PVP_DEFAULT_BET", Bet::literal(1));
        let check_acceptor_length = get_env_value_or_default("PVP_CHECK_ACCEPTOR_LENGTH", false);
        let callback_locks = get_env_value_or_default("PVP_CALLBACK_LOCKS_ENABLED", true);
        let show_stats = get_env_value_or_default("PVP_STATS_SHOW", true);
        let show_stats_notice = get_env_value_or_default("PVP_STATS_SHOW_NOTICE", true);
        let announcement_max_shows = get_optional_env_value("ANNOUNCEMENT_MAX_SHOWS");
        let announcement_en = get_optional_env_value("ANNOUNCEMENT_EN");
        let announcement_ru = get_optional_env_value("ANNOUNCEMENT_RU");
        let announcement_it = get_optional_env_value("ANNOUNCEMENT_IT");
        let announcement_fa = get_optional_env_value("ANNOUNCEMENT_FA");
        let announcement_zh = get_optional_env_value("ANNOUNCEMENT_ZH");
        let self_destruction = SelfDestructionConfig {
            notice: get_optional_env_minutes("MSG_SELFDESTRUCT_DELAY_NOTICE"),
            report: get_optional_env_minutes("MSG_SELFDESTRUCT_DELAY_REPORT"),
            reading_speed_cpm: get_env_value_or_default("MSG_SELFDESTRUCT_READING_SPEED_CPM", 500),
            warning: Duration::from_secs(get_optional_env_value("MSG_SELFDESTRUCT_WARNING_SECONDS")),
        };
        Self {
            features: FeatureToggles {
                chats_merging,
                top_unlimited,
                multiple_loans,
                dod_selection_mode,
                pvp: BattlesFeatureToggles {
                    check_acceptor_length,
                    callback_locks,
                    show_stats,
                    show_stats_notice,
                }
            },
            top_limit,
            inactivity_days,
            loan_payout_ratio,
            dod_rich_exclusion_ratio,
            pvp_default_bet,
            announcements: AnnouncementsConfig {
                max_shows: announcement_max_shows,
                announcements: [
                    (EN, announcement_en),
                    (RU, announcement_ru),
                    (IT, announcement_it),
                    (FA, announcement_fa),
                    (ZH, announcement_zh),
                ].map(|(lc, text)| (lc, Announcement::new(text)))
                    .into_iter()
                    .filter_map(|(lc, mb_ann)| mb_ann.map(|ann| (lc, ann)))
                    .collect()
            },
            self_destruction,
            command_toggles: Default::default(),
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
