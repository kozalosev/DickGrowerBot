mod shrink;

use teloxide::Bot;
use teloxide::adaptors::throttle::{Limits, Throttle};
use crate::config::{get_env_value_or_default, AppConfig};
use crate::handlers::utils::date::duration_till_next_day;
use crate::repo::Repositories;
use crate::users::LanguageService;
use shrink::run_daily_shrink;

/// Spawns a detached, best-effort task that runs the daily shrink at every UTC midnight. No-op when
/// the feature is disabled. Like the self-destruction scheduler, it isn't persisted — a restart just
/// resumes from the next midnight; failures are logged and never abort the loop.
pub fn spawn_daily_shrink(
    bot: Bot,
    repos: Repositories,
    language_service: LanguageService,
    config: AppConfig,
) {
    if !config.stale_dicks_shrinking.enabled() {
        log::info!("the daily shrink is disabled (set SHRINK_RATIO and SHRINK_GRACE_DAYS to enable it)");
        return;
    }
    tokio::spawn(async move {
        // The broadcast fans out to every affected chat at once, which would blow past Telegram's
        // rate limits unthrottled. Scoped to this task, so the dispatcher and handlers keep the
        // plain `Bot`. The adapter's queue lives in memory only: a restart mid-broadcast drops the
        // notifications still pending, and there's no resume.
        let bot = Throttle::new_spawn(bot, Limits::default());
        // Puts the feature into effect at once instead of hours later. Re-running the same day is
        // harmless: nothing here touches `updated_at`, so the repeat picks the same victims and
        // aborts on Stale_Dick_Shrinks' primary key, rolling the length change back with it.
        if get_env_value_or_default("SHRINK_RUN_ON_STARTUP", false) {
            log::warn!("SHRINK_RUN_ON_STARTUP is set — running the daily shrink right now");
            run_daily_shrink(bot.clone(), repos.clone(), language_service.clone(), config.clone())
                .await.unwrap_or_else(|e| log::error!("the daily shrink run failed: {e:#}"))
        }
        loop {
            let Some(till_next_day) = duration_till_next_day().and_then(|d| d.to_std().ok()) else {
                log::error!("couldn't compute a valid duration till the next UTC midnight, stopping the daily shrink scheduler");
                return;
            };
            tokio::time::sleep(till_next_day).await;

            run_daily_shrink(bot.clone(), repos.clone(), language_service.clone(), config.clone())
                .await.unwrap_or_else(|e| log::error!("the daily shrink run failed: {e:#}"))
        }
    });
}
