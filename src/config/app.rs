use std::time::Duration;
use reqwest::Url;
use crate::config::env::*;
use crate::config::toggles::*;
use crate::config::announcements::*;
use crate::config::self_destruction::*;
use crate::config::shrink::DailyShrinkConfig;
use crate::config::incrementor::IncrementorConfig;
use crate::domain::primitives::{AttemptsCount, Bet, DaysCount, Limit, Ratio};
use crate::domain::primitives::chat::TelegramChatId;

#[derive(Clone)]
#[cfg_attr(test, derive(Default))]
pub struct AppConfig {
    pub features: FeatureToggles,
    pub top_limit: Limit,
    pub inactivity_days: DaysCount,
    pub loan_payout_ratio: Ratio,
    pub dod_rich_exclusion_ratio: Option<Ratio>,
    pub pvp_default_bet: Bet,
    pub incrementor: IncrementorConfig,
    pub daily_shrink: DailyShrinkConfig,
    pub announcements: AnnouncementsConfig,
    pub self_destruction: SelfDestructionConfig,
    pub command_toggles: CachedEnvToggles,
    pub support_chat_id: Option<TelegramChatId>,
    pub ban_list_refresh_secs: Duration,
}

#[derive(Clone)]
pub struct DatabaseConfig {
    pub url: Url,
    pub max_connections: u32,
    pub min_connections: u32,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let top_limit = env_value!("TOP_LIMIT": Limit, or = 10);
        let inactivity_days = env_value!("INACTIVITY_DAYS": DaysCount, or = 7);
        let loan_payout_ratio = env_value!("LOAN_PAYOUT_COEF": Ratio);
        let dod_selection_mode = get_optional_env_value("DOD_SELECTION_MODE");
        let dod_rich_exclusion_ratio = get_optional_env_ratio("DOD_RICH_EXCLUSION_RATIO");
        let chats_merging = get_env_value_or_default("CHATS_MERGING_ENABLED", false);
        let top_unlimited = get_env_value_or_default("TOP_UNLIMITED_ENABLED", false);
        let multiple_loans = get_env_value_or_default("MULTIPLE_LOANS_ENABLED", false);
        let pvp_default_bet = env_value!("PVP_DEFAULT_BET": Bet, or = 1);
        let check_acceptor_length = get_env_value_or_default("PVP_CHECK_ACCEPTOR_LENGTH", false);
        let callback_locks = get_env_value_or_default("PVP_CALLBACK_LOCKS_ENABLED", true);
        let show_stats = get_env_value_or_default("PVP_STATS_SHOW", true);
        let show_stats_notice = get_env_value_or_default("PVP_STATS_SHOW_NOTICE", true);
        let most_popular_language_enabled = get_env_value_or_default("MOST_POPULAR_LANGUAGE_ENABLED", true);
        let hide_inactive_zero_length_from_top = get_env_value_or_default("HIDE_INACTIVE_ZERO_LENGTH_FROM_TOP", true);
        let daily_shrink = DailyShrinkConfig {
            ratio: env_value!("DAILY_SHRINK_RATIO": Ratio),
            inactivity_days: env_value!("DAILY_SHRINK_INACTIVITY_DAYS": DaysCount, or = 7),
            ramp_up_days: env_value!("DAILY_SHRINK_RAMP_UP_DAYS": DaysCount, or = 7),
        };
        let announcements_file = get_env_value_or_default("ANNOUNCEMENTS_FILE", "announcements.yml".to_string());
        let self_destruction = SelfDestructionConfig {
            notice: EnvDuration::minutes("MSG_SELFDESTRUCT_DELAY_NOTICE").read(),
            report: EnvDuration::minutes("MSG_SELFDESTRUCT_DELAY_REPORT").read(),
            event: EnvDuration::minutes("MSG_SELFDESTRUCT_DELAY_EVENT").read(),
            application: EnvDuration::minutes("MSG_SELFDESTRUCT_DELAY_APPLICATION").read(),
            reading_speed_cpm: get_env_value_or_default("MSG_SELFDESTRUCT_READING_SPEED_CPM", 500),
            warning: EnvDuration::seconds("MSG_SELFDESTRUCT_WARNING_SECONDS").read(),
            mode: get_optional_env_value("MSG_SELFDESTRUCT_MODE"),
            poll_interval: EnvDuration::seconds("MSG_SELFDESTRUCT_POLL_SECS").or(5).at_least(1).read(),
            batch_size: env_value!("MSG_SELFDESTRUCT_BATCH_SIZE": Limit, or = 50, at_least = 1),
            lease: EnvDuration::seconds("MSG_SELFDESTRUCT_LEASE_SECS").or(300).at_least(1).read(),
            inline_groups: get_optional_env_value("MSG_SELFDESTRUCT_INLINE_GROUPS"),
            retry_delay: EnvDuration::seconds("MSG_SELFDESTRUCT_RETRY_DELAY_SECONDS").or(60).at_least(1).read(),
            max_retry_delay: EnvDuration::seconds("MSG_SELFDESTRUCT_MAX_RETRY_DELAY_SECS").or(3600).at_least(1).read(),
            max_attempts: env_value!("MSG_SELFDESTRUCT_MAX_ATTEMPTS": AttemptsCount, or = 3, at_least = 1),
            retention: EnvDuration::minutes("MSG_SELFDESTRUCT_TABLE_CLEANING_DELAY").or(1440).read(),
        };
        let support_chat_id = get_optional_chat_id("SUPPORT_CHAT_ID");
        let ban_list_refresh_secs = EnvDuration::seconds("BAN_LIST_REFRESH_SECS").or(900).at_least(1).read();
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
                },
                most_popular_language_enabled,
                hide_inactive_zero_length_from_top,
            },
            top_limit,
            inactivity_days,
            loan_payout_ratio,
            dod_rich_exclusion_ratio,
            pvp_default_bet,
            incrementor: IncrementorConfig::from_env(),
            daily_shrink,
            announcements: AnnouncementsConfig::load(&announcements_file),
            self_destruction,
            command_toggles: Default::default(),
            support_chat_id,
            ban_list_refresh_secs,
        }
    }
}

impl DatabaseConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            url: get_env_mandatory_value("DATABASE_URL")?,
            max_connections: get_env_value_or_default("DATABASE_MAX_CONNECTIONS", 10),
            min_connections: get_env_value_or_default("DATABASE_MIN_CONNECTIONS", 5),
        })
    }
}
