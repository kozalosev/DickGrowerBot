mod shrink;

use teloxide::Bot;
use crate::config::AppConfig;
use crate::handlers::utils::date::duration_till_next_day_utc;
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
        loop {
            let till_next_day = duration_till_next_day_utc()
                .to_std()
                .unwrap_or(std::time::Duration::ZERO);
            tokio::time::sleep(till_next_day).await;

            run_daily_shrink(bot.clone(), repos.clone(), language_service.clone(), config.clone())
                .await.unwrap_or_else(|e| log::error!("the daily shrink run failed: {e:#}"))
        }
    });
}
