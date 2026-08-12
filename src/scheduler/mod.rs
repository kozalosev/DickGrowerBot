mod shrink;
mod deletions;
mod broadcasts;

use std::time::Duration;
use teloxide::Bot;
use teloxide::adaptors::throttle::{Settings, Throttle};
use domain_types::traits::SaturatingInto;
use crate::cache::Cache;
use crate::config::{get_env_value_or_default, AppConfig, ThrottleConfig};
use crate::domain::primitives::AttemptsCount;
use crate::handlers::utils::date::duration_till_next_day;
use crate::metrics;
use crate::repo::Repositories;
use crate::topics::TopicPolicy;
use crate::users::LanguageService;
use shrink::run_daily_shrink;
use deletions::{clean_finished_deletions, run_pending_deletions};
use broadcasts::{clean_finished_broadcasts, run_pending_broadcasts, BroadcastDeps};

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
/// the feature is disabled. The run itself isn't persisted — a restart just resumes from the next
/// midnight; failures are logged and never abort the loop.
///
/// What the run produces *is* persisted: the summaries it owes are rows, written by the same
/// statement that shrank the dicks, so nothing here has to survive for a chat to be notified.
pub fn spawn_daily_shrink(repos: Repositories, config: AppConfig) {
    if !config.daily_shrink.enabled() {
        tracing::info!("the daily shrink is disabled (set DAILY_SHRINK_RATIO and DAILY_SHRINK_INACTIVITY_DAYS to enable it)");
        return;
    }
    tokio::spawn(metrics::TASK_DAILY_SHRINK.instrument(async move {
        // A failed run is logged and forgotten: the next midnight tries again, and one bad day
        // must not stop the scheduler for good.
        let run = || async {
            run_daily_shrink(repos.clone(), config.clone())
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

/// Spawns the task that sends the shrink summaries the chats are owed. No-op when the daily shrink
/// is disabled, since nothing would ever write a row.
///
/// Unlike the shrink above, this one survives a restart: what it acts on are rows, and the tick
/// after the restart claims every summary that fell due meanwhile — which is what makes a broadcast
/// to a few hundred thousand chats possible at all.
pub fn spawn_broadcast_worker(
    bot: Throttle<Bot>,
    repos: Repositories,
    language_service: LanguageService,
    topics: TopicPolicy,
    config: AppConfig,
) {
    if !config.daily_shrink.enabled() {
        return;
    }
    // Published so that a graph of the batch size can be read against the limit it may reach,
    // instead of against a number written into the dashboard.
    metrics::DAILY_SHRINK_BROADCAST_BATCH_LIMIT.set(i64::from(config.daily_shrink.broadcast.batch_size));
    tokio::spawn(metrics::TASK_DAILY_SHRINK_BROADCAST.instrument(async move {
        let mut ticker = tokio::time::interval(config.daily_shrink.broadcast.poll_interval);
        loop {
            ticker.tick().await;

            // A failed tick is logged and forgotten: the rows are still there, and the next tick
            // picks them up. Only the count is skipped, as it comes from the same database.
            let deps = BroadcastDeps {
                bot: &bot, repos: &repos, language_service: &language_service,
                topics: &topics, config: &config,
            };
            if let Err(e) = run_pending_broadcasts(deps).await {
                tracing::error!(error = format!("{e:#}"), "a shrink broadcast run failed");
                continue;
            }
            report_shrink_queue(&repos).await;
        }
    }));
}

/// Spawns the task that clears the finished rows out of the broadcast queue's table. Separate from
/// the worker so that the history of what it did can be kept (and read) for as long as the
/// retention says — zero keeps it for ever, which is what to set while debugging the worker itself.
pub fn spawn_broadcast_cleaner(repos: Repositories, config: AppConfig) {
    let retention = config.daily_shrink.broadcast.retention;
    if !config.daily_shrink.enabled() || retention.is_zero() {
        return;
    }
    tokio::spawn(metrics::TASK_DAILY_SHRINK_BROADCAST_CLEANING.instrument(async move {
        // Runs as often as it keeps, so a row lives between one and two retention periods. There's
        // nothing to gain from looking more often: nothing becomes stale in between.
        let mut ticker = tokio::time::interval(retention);
        loop {
            ticker.tick().await;

            clean_finished_broadcasts(&repos, retention).await
                .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "the cleaning of the finished shrink summaries failed"));
        }
    }));
}

/// Spawns the task that removes the messages whose self-destruction has come due. No-op when every
/// group is permanent. Unlike the two schedulers above, this one survives a restart: the messages
/// it acts on are rows, and the tick after the restart finds every one that fell due meanwhile.
pub fn spawn_deletion_worker(bot: Throttle<Bot>, repos: Repositories, cache: Cache, config: AppConfig) {
    let self_destruction = config.self_destruction;
    let bot_admin_ttl = config.caches.bot_admin;
    if !self_destruction.enabled() {
        tracing::info!("the self-destruction of messages is disabled (set the MSG_SELFDESTRUCT_DELAY_* variables to enable it)");
        return;
    }
    // Published so that a graph of the batch size can be read against the limit it may reach,
    // instead of against a number written into the dashboard.
    metrics::SELF_DESTRUCTION_BATCH_LIMIT.set(i64::from(self_destruction.batch_size));
    tokio::spawn(metrics::TASK_SELF_DESTRUCTION.instrument(async move {
        let mut ticker = tokio::time::interval(self_destruction.poll_interval);
        loop {
            ticker.tick().await;

            // A failed tick is logged and forgotten: the rows are still there, and the next tick
            // picks them up. Only the count is skipped, as it comes from the same database.
            if let Err(e) = run_pending_deletions(&bot, &repos, &cache, &self_destruction, bot_admin_ttl).await {
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

/// How long a row rests before the next attempt: the base delay doubled once per failure it already
/// has, up to `max`. A chat that answers slowly is asked less and less often, instead of at a steady
/// beat for as many attempts as it is given.
///
/// Shared by both queues, which want the same curve out of the same three settings.
fn backoff(base: Duration, failures: AttemptsCount, max: Duration) -> Duration {
    let factor = 1u32.checked_shl(failures.value()).unwrap_or(u32::MAX);
    base.saturating_mul(factor).min(max)
}

/// Publishes how much the broadcast queue still owes and when the last shrink was.
///
/// Only these two, and only from the worker's tick: they are what the alerts read, and vmalert
/// can't ask the database itself. Everything a human looks at — which chats failed and why, the
/// states over time — is a panel over `Scheduled_Shrink_Broadcasts` instead, which costs nothing
/// when nobody is looking at it.
///
/// The day of the last shrink is a gauge rather than a counter because it has to survive a restart:
/// a counter incremented once a day reads zero both when nothing happened and when nobody scraped
/// it in time, and there is no telling those apart afterwards.
///
/// A failure here loses one sample of a gauge and nothing else — the queue, the rows and the worker
/// are untouched, and the next tick publishes again — so it is a `warn`. It also runs every few
/// seconds, and a database that is down has already been reported by the run that failed.
async fn report_shrink_queue(repos: &Repositories) {
    match repos.broadcasts.count_pending().await {
        Ok(pending) => metrics::DAILY_SHRINK_BROADCAST_PENDING.set(pending.saturating_into()),
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't count the pending shrink summaries"),
    }
    match repos.shrinks.get_last_shrink_timestamp().await {
        Ok(Some(at)) => metrics::DAILY_SHRINK_LAST_RUN_TIMESTAMP.set(at.timestamp()),
        Ok(None) => {},
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't read the time of the last shrink"),
    }
}

/// Publishes the depth of the queue and of its backlog of finished rows. Both are read from the
/// same database the run just used, so a failure here is only logged.
async fn report_queue(repos: &Repositories) {
    match repos.deletions.count_pending().await {
        Ok(pending) => metrics::SELF_DESTRUCTION_PENDING.set(pending.saturating_into()),
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't count the pending self-destructions"),
    }
    match repos.deletions.count_finished().await {
        Ok(finished) => metrics::SELF_DESTRUCTION_FINISHED.set_all(&finished),
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't count the finished self-destructions"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: Duration = Duration::from_hours(1);

    #[test]
    fn the_back_off_doubles_with_every_failure() {
        let base = Duration::from_secs(60);
        assert_eq!(backoff(base, AttemptsCount::new(0), MAX), Duration::from_secs(60));
        assert_eq!(backoff(base, AttemptsCount::new(1), MAX), Duration::from_secs(120));
        assert_eq!(backoff(base, AttemptsCount::new(2), MAX), Duration::from_secs(240));
        assert_eq!(backoff(base, AttemptsCount::new(3), MAX), Duration::from_secs(480));
    }

    #[test]
    fn the_back_off_never_grows_past_its_cap() {
        let base = Duration::from_secs(60);
        assert_eq!(backoff(base, AttemptsCount::new(30), MAX), MAX);
        // The shift that would overflow must give the cap, not a wrapped-around delay of nothing.
        assert_eq!(backoff(base, AttemptsCount::new(u32::MAX), MAX), MAX);
    }
}
