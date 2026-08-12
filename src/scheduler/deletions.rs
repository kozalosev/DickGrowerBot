use std::time::Duration;
use autometrics::autometrics;
use chrono::Utc;
use futures::{stream, StreamExt};
use rust_i18n::t;
use teloxide::{ApiError, Bot, RequestError};
use teloxide::adaptors::Throttle;
use teloxide::payloads::{EditMessageTextInlineSetters, EditMessageTextSetters};
use teloxide::requests::Requester;
use teloxide::types::{ChatId, MessageId};
use teloxide::types::ParseMode::Html;
use crate::config::SelfDestructionConfig;
use crate::config::MessageGroup;
use domain_types::traits::ApproxInto;
use crate::cache::Cache;
use crate::handlers::rights;
use crate::domain::primitives::ScheduledDeletionId;
use crate::metrics;
use crate::repo::{DeletionState, DeletionTarget, MessageKind, Repositories, ScheduledDeletion};
use super::backoff;

/// The age at which Telegram stops letting a bot delete a message. This is the real limit;
/// [`crate::config::MAX_DELAY`] caps the delays an hour below it. Nothing is scheduled this late,
/// so a message gets here only if the queue fell behind. It is a constant and not a setting,
/// because the number is Telegram's and our own value would change nothing.
const MAX_AGE: Duration = Duration::from_secs(48 * 60 * 60);

/// What the worker decided to do with a row once it had acted on its message.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The message is dealt with — deleted or replaced by its placeholder.
    Removed,
    /// It was already gone when its turn came.
    RemovedBefore,
    /// The message now carries the warning; it comes back at the end of the grace period.
    Warned,
    /// Something transient went wrong; the row is tried again later.
    Retry,
    /// The message grew too old for Telegram to delete while it waited.
    Expired,
    /// It won't work, now or later: the bot may not touch that message, or the chat is gone.
    Failed,
}

/// Takes one batch of messages whose time has come and acts on each of them.
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn run_pending_deletions(
    bot: &Throttle<Bot>,
    repos: &Repositories,
    cache: &Cache,
    config: &SelfDestructionConfig,
    bot_admin_ttl: Duration,
) -> anyhow::Result<()> {
    let due = repos.deletions.claim_due(config.batch_size, Utc::now() + config.lease).await?;
    // The empty runs are measured too: an idle worker is what tells a queue that keeps up from one
    // that is merely being asked for less than it holds.
    metrics::SELF_DESTRUCTION_BATCH_SIZE.observe(due.len().approx_into());
    if due.is_empty() {
        return Ok(())
    }
    tracing::debug!(count = due.len(), "acting on the messages due for self-destruction");

    // Concurrently, because what one run gets through would otherwise be one message per round
    // trip to Telegram however large the batch. `delete_message` and `edit_message_text` are not
    // subject to the limits on sending — teloxide's `Throttle` passes both straight through — so
    // the bound here is our own connection and database pools, not Telegram's patience. If that
    // turns out to be wrong, a 429 is not final: the row is postponed and tried again, and it
    // shows up as `telegram_request_errors_total{kind="rate_limited"}`.
    stream::iter(due)
        .for_each_concurrent(usize::from(config.concurrency), |deletion| async move {
            act_and_record(bot, repos, cache, config, bot_admin_ttl, deletion).await
        })
        .await;
    Ok(())
}

/// Acts on one message and writes down what became of it.
async fn act_and_record(
    bot: &Throttle<Bot>,
    repos: &Repositories,
    cache: &Cache,
    config: &SelfDestructionConfig,
    bot_admin_ttl: Duration,
    deletion: ScheduledDeletion,
) {
    let id = deletion.id;
    let group = deletion.group;
    let kind = deletion.kind;
    let failures = deletion.attempts;
    let outcome = act(bot, cache, config, bot_admin_ttl, deletion).await;
    tracing::debug!(id = %id, group = %group, kind = %kind, ?outcome, "the message is dealt with");

    // Only an ending is counted, and each one only once, so the outcomes add up to the number of
    // messages. A warning and a retry are steps, not endings; retries have their own counter.
    let result = match outcome {
        Outcome::Removed => finish(repos, id, group, kind, DeletionState::Removed).await,
        Outcome::RemovedBefore => finish(repos, id, group, kind, DeletionState::RemovedBefore).await,
        Outcome::Expired => finish(repos, id, group, kind, DeletionState::Expired).await,
        Outcome::Failed => finish(repos, id, group, kind, DeletionState::Failed).await,
        Outcome::Warned => repos.deletions.mark_warned(id, Utc::now() + config.warning).await,
        Outcome::Retry => {
            metrics::SELF_DESTRUCTION_RETRIES.record(group, kind);
            let next_attempt = Utc::now() + backoff(config.retry_delay, failures, config.max_retry_delay);
            match repos.deletions.postpone(id, next_attempt).await {
                Ok(attempts) if attempts >= config.max_attempts => {
                    tracing::warn!(id = %id, attempts = %attempts, "giving up on a self-destructing message");
                    finish(repos, id, group, kind, DeletionState::Failed).await
                },
                other => other.map(|_| ()),
            }
        },
    };
    if let Err(e) = result {
        tracing::error!(id = %id, error = format!("{e:#}"), "couldn't record the outcome of a self-destruction");
    }
}

/// Stores the state a row ended in and counts that ending. Both happen here, so the table and the
/// counter always say the same thing.
async fn finish(
    repos: &Repositories,
    id: ScheduledDeletionId,
    group: MessageGroup,
    kind: MessageKind,
    state: DeletionState,
) -> anyhow::Result<()> {
    metrics::SELF_DESTRUCTION.record(group, kind, state);
    repos.deletions.finish(id, state).await
}

/// Removes the rows that were finished long enough ago, and reports what is left.
///
/// Separate from the worker on purpose: the finished rows are the only account of what the worker
/// did, so how long they are kept is a decision of its own, and clearing them must never be part of
/// the run that produced them.
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn clean_finished_deletions(repos: &Repositories, retention: Duration) -> anyhow::Result<()> {
    let older_than = Utc::now() - retention;
    tracing::debug!(%older_than, ?retention, "cleaning the finished self-destructions up");
    let removed = repos.deletions.delete_finished(older_than).await?;
    if removed > 0 {
        tracing::info!(removed, "cleaned the finished self-destructions up");
    }
    Ok(())
}

/// Does to the message whatever its row asks for, and says what became of it.
async fn act(
    bot: &Throttle<Bot>,
    cache: &Cache,
    config: &SelfDestructionConfig,
    bot_admin_ttl: Duration,
    deletion: ScheduledDeletion,
) -> Outcome {
    // Telegram refuses to delete a message older than 48 hours. Delays are capped below that, so a
    // message can only get here if the queue itself fell behind — retries, a long outage — and the
    // row is kept as `expired` rather than spent on a request that is certain to be refused. An
    // inline message is edited, not deleted, and no age limit applies to that.
    let is_chat_message = matches!(deletion.target, DeletionTarget::ChatMessage { .. });
    let age = (Utc::now() - deletion.created_at).to_std().unwrap_or(Duration::ZERO);
    if is_chat_message && age > MAX_AGE {
        tracing::warn!(kind = %deletion.kind, created_at = %deletion.created_at,
            "the message got too old to be deleted while it waited");
        return Outcome::Expired
    }

    match &deletion.target {
        DeletionTarget::InlineMessage(inline_message_id) => {
            let placeholder = t!("self_destruction.placeholder", locale = &deletion.lang_code);
            let request = bot.edit_message_text_inline(inline_message_id.value(), placeholder)
                .parse_mode(Html);
            outcome_of(request.await.map(|_| ()), &deletion, cache, bot_admin_ttl).await
        },
        DeletionTarget::ChatMessage { chat_id, message_id } => {
            let chat = ChatId::from(*chat_id);
            let message = MessageId::from(*message_id);
            // A command belongs to its sender, so it can't be edited into anything — it is only
            // ever deleted, at the moment its answer's warning runs out.
            let warn = !config.warning.is_zero()
                && deletion.state == DeletionState::Created
                && deletion.kind != MessageKind::Command;
            if warn {
                let notice = t!("self_destruction.warning", locale = &deletion.lang_code,
                                seconds = config.warning.as_secs());
                let request = bot.edit_message_text(chat, message, notice).parse_mode(Html);
                return outcome_of_warning(request.await.map(|_| ()), chat, message)
            }
            outcome_of(bot.delete_message(chat, message).await.map(|_| ()), &deletion, cache, bot_admin_ttl).await
        },
    }
}

/// Turns the answer of the Bot API into an outcome, remembering what it says about the chat.
async fn outcome_of(
    result: Result<(), RequestError>,
    deletion: &ScheduledDeletion,
    cache: &Cache,
    bot_admin_ttl: Duration,
) -> Outcome {
    let error = match result {
        Ok(()) => return Outcome::Removed,
        Err(e) => e,
    };

    // A message that is already gone is the outcome we were after, reached by someone else.
    if is_gone(&error) {
        tracing::debug!(kind = %deletion.kind, error = %error, "the message was already gone");
        return Outcome::RemovedBefore
    }

    if is_refused(&error) && deletion.kind == MessageKind::Command {
        // The bot may not delete messages here. Remembering that keeps every later command out of
        // the queue instead of into this same failure.
        if let DeletionTarget::ChatMessage { chat_id, .. } = deletion.target {
            rights::remember_deletion_right(cache, ChatId::from(chat_id), false, bot_admin_ttl).await;
        }
    }
    if is_final(&error) {
        tracing::warn!(kind = %deletion.kind, error = %error, "the message can't be removed at all");
        return Outcome::Failed
    }

    tracing::warn!(kind = %deletion.kind, error = %error, "couldn't remove the self-destructing message");
    Outcome::Retry
}

/// What the warning attempt means for the row.
///
/// A message that is already gone will not come back, so the row ends here rather than a grace
/// period and a certain-to-fail deletion later. Any other failure carries on: a message that could
/// not be warned is still worth deleting on time, and the notice is the part worth losing.
fn outcome_of_warning(result: Result<(), RequestError>, chat: ChatId, message: MessageId) -> Outcome {
    let error = match result {
        Ok(()) => return Outcome::Warned,
        Err(e) => e,
    };
    if is_gone(&error) {
        tracing::debug!(chat_id = %chat, message_id = %message, "the message was gone before its warning");
        return Outcome::RemovedBefore
    }
    tracing::warn!(chat_id = %chat, message_id = %message, error = %error,
        "couldn't edit the message with the warning");
    Outcome::Warned
}

/// The wordings Telegram uses for a bot that may not delete that message and that teloxide has no
/// variant for. The raw text is all there is to look at, as with the unreachable chats of the
/// daily shrink.
const REFUSAL_ERROR_TEXTS: [&str; 2] = [
    "rights to delete",
    "rights to manage messages",
];

/// Whether Telegram refused because the bot isn't allowed to touch that message.
fn is_refused(error: &RequestError) -> bool {
    match error {
        RequestError::Api(ApiError::MessageCantBeDeleted) => true,
        RequestError::Api(ApiError::Unknown(text)) =>
            REFUSAL_ERROR_TEXTS.iter().any(|refusal| text.contains(refusal)),
        _ => false,
    }
}

/// Whether the message is already gone — the outcome we were after, reached by someone else.
fn is_gone(error: &RequestError) -> bool {
    matches!(error, RequestError::Api(
        ApiError::MessageToDeleteNotFound | ApiError::MessageToEditNotFound | ApiError::MessageIdInvalid))
}

/// Whether retrying could ever help. Everything Telegram says about the message itself, about the
/// bot's rights or about the chat is final — only network hiccups and rate limits are worth
/// another attempt.
fn is_final(error: &RequestError) -> bool {
    if is_gone(error) || is_refused(error) {
        return true
    }
    matches!(error, RequestError::Api(
        ApiError::MessageNotModified
        | ApiError::BotBlocked
        | ApiError::BotKicked
        | ApiError::BotKickedFromSupergroup
        | ApiError::BotKickedFromChannel
        | ApiError::ChatNotFound
        | ApiError::GroupDeactivated
        | ApiError::UserDeactivated))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message someone else removed used to be warned first and only found missing a grace
    /// period later, at the cost of two requests and a warn-level line for a normal event.
    #[test]
    fn a_message_gone_before_its_warning_ends_there() {
        let chat = ChatId(-1001);
        let message = MessageId(1);
        for gone in [ApiError::MessageToEditNotFound, ApiError::MessageToDeleteNotFound, ApiError::MessageIdInvalid] {
            let outcome = outcome_of_warning(Err(RequestError::Api(gone)), chat, message);
            assert_eq!(outcome, Outcome::RemovedBefore);
        }
    }

    /// Anything else is transient as far as the warning is concerned: the notice is lost, the
    /// deletion is not.
    #[test]
    fn a_warning_that_merely_failed_still_leads_to_a_deletion() {
        let chat = ChatId(-1001);
        let message = MessageId(1);
        assert_eq!(outcome_of_warning(Ok(()), chat, message), Outcome::Warned);

        let transient = RequestError::Api(ApiError::Unknown("Bad Gateway".to_owned()));
        assert_eq!(outcome_of_warning(Err(transient), chat, message), Outcome::Warned);

        let rate_limited = RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(5));
        assert_eq!(outcome_of_warning(Err(rate_limited), chat, message), Outcome::Warned);
    }

    #[test]
    fn a_message_that_is_gone_is_not_retried() {
        let gone = RequestError::Api(ApiError::MessageToDeleteNotFound);
        assert!(is_gone(&gone));
        assert!(is_final(&gone));
    }

    #[test]
    fn a_refused_deletion_is_final_but_not_a_success() {
        let refused = RequestError::Api(ApiError::MessageCantBeDeleted);
        assert!(is_refused(&refused));
        assert!(is_final(&refused));
        assert!(!is_gone(&refused));
    }

    #[test]
    fn a_refusal_telegram_words_its_own_way_is_recognized() {
        let refused = RequestError::Api(ApiError::Unknown(
            "Bad Request: not enough rights to delete a message".to_owned()));
        assert!(is_refused(&refused));
        assert!(is_final(&refused));
    }

    #[test]
    fn a_rate_limit_is_worth_another_attempt() {
        let rate_limited = RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(5));
        assert!(!is_final(&rate_limited));
        assert!(!is_refused(&rate_limited));
    }
}
