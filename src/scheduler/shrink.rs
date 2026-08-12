use autometrics::autometrics;
use crate::config::AppConfig;
use crate::metrics;
use crate::repo::{Repositories, ShrinkBatchOutcome};

/// Runs the daily shrink: applies the decay to every stale dick and queues one summary per chat the
/// bot can post to. Nothing is sent from here — the queue's worker does that, at its own pace.
///
/// The chats are walked in batches, so neither the locks nor a failure covers more than a batch,
/// and the run returns in minutes however many chats there are. That is what keeps the scheduler's
/// loop able to reach the next midnight.
///
/// Inline-only chats (no messageable `chat_id`) get no summary — their members see the events via
/// the `shrinks` inline command — and neither do the ones the bot is known to have lost access to.
/// Both are counted, under the `inline_only` and `unreachable` labels of [`metrics::DAILY_SHRINK`].
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn run_daily_shrink(repos: Repositories, config: AppConfig) -> anyhow::Result<()> {
    let shrink_config = &config.daily_shrink;
    let mut total = ShrinkBatchOutcome::default();
    let mut chats = 0usize;
    let mut failed_batches = 0u32;
    let mut after = None;

    // A page of chats is read, shrunk and forgotten before the next one is asked for, so the run
    // holds one page at a time however many chats there are.
    loop {
        let page = repos.shrinks.select_chats_page(after, shrink_config.batch_size)
            .await
            .inspect_err(|_| metrics::DAILY_SHRINK.run_failed())?;
        let Some(last) = page.last().copied() else { break };
        after = Some(last);
        chats += page.len();

        match repos.shrinks.perform_daily_shrink(&page, shrink_config.ratio,
                                                 shrink_config.inactivity_days, shrink_config.ramp_up_days).await {
            // A batch that fails takes its own chats down and nothing else: the ones already
            // shrunk keep their summaries, and the rest are still ahead. Nothing retries it, so
            // those chats have lost the day — an error, not a warning.
            Err(e) => {
                failed_batches += 1;
                tracing::error!(chats = page.len(), error = format!("{e:#}"), "a batch of the daily shrink failed");
            },
            Ok(outcome) => total += outcome,
        }
    }

    metrics::DAILY_SHRINK.victims_to_broadcast(total.to_broadcast.value());
    metrics::DAILY_SHRINK.victims_inline_only(total.inline_only.value());
    metrics::DAILY_SHRINK.victims_unreachable(total.unreachable.value());
    metrics::DAILY_SHRINK.broadcast_skipped(total.chats_skipped.value());

    // A run that lost a batch is a failed run even if the others went through: a partial day must
    // not read like a whole one. Each batch has already said so in its own line, with the error;
    // this only counts the day.
    if failed_batches > 0 {
        metrics::DAILY_SHRINK.run_failed();
    } else if total.victims.value() == 0 {
        metrics::DAILY_SHRINK.run_empty();
        tracing::info!(chats, "nothing to shrink today");
        return Ok(())
    } else {
        metrics::DAILY_SHRINK.run_succeeded();
    }
    tracing::info!(chats, failed_batches, victims = total.victims.value(),
        queued = total.chats_queued.value(), skipped = total.chats_skipped.value(),
        "the daily shrink is done");
    Ok(())
}
