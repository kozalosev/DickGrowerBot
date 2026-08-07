mod shrink;
mod deletions;

use teloxide::Bot;
use teloxide::adaptors::throttle::{Settings, Throttle};
use crate::config::{get_env_value_or_default, AppConfig, ThrottleConfig};
use crate::handlers::utils::date::duration_till_next_day;
use crate::metrics;
use crate::repo::Repositories;
use crate::topics::TopicPolicy;
use crate::users::LanguageService;
use shrink::run_daily_shrink;
use deletions::{clean_finished_deletions, run_pending_deletions};

/// A bot that keeps the schedulers inside Telegram's rate limits.
///
/// Both schedulers send to many chats at once, so they need this. They must also share one, because
/// the counting happens in a worker task the wrapper spawns, and two of them would count two
/// budgets and allow twice as much. A clone of this shares the same worker.
///
/// The dispatcher and the handlers keep the plain `Bot`: they answer one user at a time.
pub fn throttled(bot: Bot, config: ThrottleConfig) -> Throttle<Bot> {
    let settings = Settings::default()
        .limits(config.into())
        .on_queue_full(|pending| async move {
            metrics::TELEGRAM_THROTTLE_QUEUE_FULL.inc();
            tracing::warn!(pending, "the throttle queue is full, requests are waiting for their turn");
        });
    Throttle::spawn_with_settings(bot, settings)
}

/// Spawns a detached, best-effort task that runs the daily shrink at every UTC midnight. No-op when
/// the feature is disabled. Like the self-destruction scheduler, it isn't persisted — a restart just
/// resumes from the next midnight; failures are logged and never abort the loop.
pub fn spawn_daily_shrink(
    bot: Throttle<Bot>,
    repos: Repositories,
    language_service: LanguageService,
    topics: TopicPolicy,
    config: AppConfig,
) {
    if !config.daily_shrink.enabled() {
        tracing::info!("the daily shrink is disabled (set DAILY_SHRINK_RATIO and DAILY_SHRINK_INACTIVITY_DAYS to enable it)");
        return;
    }
    tokio::spawn(metrics::TASK_DAILY_SHRINK.instrument(async move {
        // The throttle queue lives in memory only: a restart during the broadcast drops the
        // notifications that still wait, and there is no way to resume them.
        // TODO: [#154] Make broadcasting of messages durable by storing tasks in the database
        //
        // A failed run is logged and forgotten: the next midnight tries again, and one bad day
        // must not stop the scheduler for good.
        let run = || async {
            run_daily_shrink(bot.clone(), repos.clone(), language_service.clone(), topics.clone(), config.clone())
                .await
                .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "the daily shrink run failed"))
        };
        // Puts the feature into effect at once instead of hours later. Re-running the same day is
        // harmless: nothing here touches `updated_at`, so the repeat picks the same victims and
        // aborts on Stale_Dick_Shrinks' primary key, rolling the length change back with it.
        if get_env_value_or_default("DAILY_SHRINK_RUN_ON_STARTUP", false) {
            tracing::warn!(variable = "DAILY_SHRINK_RUN_ON_STARTUP", "the variable is set, running the daily shrink right now");
            run().await;
        }
        loop {
            let Some(till_next_day) = duration_till_next_day().and_then(|d| d.to_std().ok()) else {
                tracing::error!("couldn't compute a valid duration till the next UTC midnight, stopping the daily shrink scheduler");
                return;
            };
            tokio::time::sleep(till_next_day).await;

            run().await;
        }
    }));
}

/// Spawns the task that removes the messages whose self-destruction has come due. No-op when every
/// group is permanent. Unlike the two schedulers above, this one survives a restart: the messages
/// it acts on are rows, and the tick after the restart finds every one that fell due meanwhile.
pub fn spawn_deletion_worker(bot: Throttle<Bot>, repos: Repositories, config: AppConfig) {
    let self_destruction = config.self_destruction;
    if !self_destruction.enabled() {
        tracing::info!("the self-destruction of messages is disabled (set the MSG_SELFDESTRUCT_DELAY_* variables to enable it)");
        return;
    }
    // Published so that a graph of the batch size can be read against the limit it may reach,
    // instead of against a number written into the dashboard.
    metrics::SELF_DESTRUCTION_BATCH_LIMIT.set(self_destruction.batch_size.value() as i64);
    tokio::spawn(metrics::TASK_SELF_DESTRUCTION.instrument(async move {
        let mut ticker = tokio::time::interval(self_destruction.poll_interval);
        loop {
            ticker.tick().await;

            // A failed tick is logged and forgotten: the rows are still there, and the next tick
            // picks them up. Only the count is skipped, as it comes from the same database.
            if let Err(e) = run_pending_deletions(&bot, &repos, &self_destruction).await {
                tracing::error!(error = format!("{e:#}"), "a self-destruction run failed");
                continue;
            }
            report_queue(&repos).await;
        }
    }));
}

/// Spawns the task that clears the finished rows out of the queue's table. Separate from the worker
/// so that the history of what it did can be kept (and read) for as long as the retention says —
/// zero keeps it for ever, which is what to set while debugging the worker itself.
pub fn spawn_deletion_cleaner(repos: Repositories, config: AppConfig) {
    let retention = config.self_destruction.retention;
    if !config.self_destruction.enabled() || retention.is_zero() {
        return;
    }
    tokio::spawn(metrics::TASK_SELF_DESTRUCTION_CLEANING.instrument(async move {
        // Runs as often as it keeps, so a row lives between one and two retention periods. There's
        // nothing to gain from looking more often: nothing becomes stale in between.
        let mut ticker = tokio::time::interval(retention);
        loop {
            ticker.tick().await;

            clean_finished_deletions(&repos, retention).await
                .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "the cleaning of the finished self-destructions failed"));
            report_queue(&repos).await;
        }
    }));
}

/// Publishes the depth of the queue and of its backlog of finished rows. Both are read from the
/// same database the run just used, so a failure here is only logged.
async fn report_queue(repos: &Repositories) {
    match repos.deletions.count_pending().await {
        Ok(pending) => metrics::SELF_DESTRUCTION_PENDING.set(pending.value() as i64),
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't count the pending self-destructions"),
    }
    match repos.deletions.count_finished().await {
        Ok(finished) => metrics::SELF_DESTRUCTION_FINISHED.set_all(&finished),
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't count the finished self-destructions"),
    }
}
