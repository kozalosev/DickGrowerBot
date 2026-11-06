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
use crate::cleanup::CleanupPolicy;
use crate::handlers::rights;
use crate::handlers::utils::resolve_inline_message_id;
use crate::domain::objects::ChatCleanupSettings;
use crate::domain::primitives::ScheduledDeletionId;
use crate::domain::primitives::chat::InlineMessageId;
use crate::metrics;
use crate::repo::{DeletionState, DeletionTarget, MessageKind, Repositories, ScheduledDeletion};
use super::backoff;

/// The age at which Telegram stops letting a bot delete a message. This is the real limit;
/// [`crate::config::MAX_DELAY`] caps the delays an hour below it. Nothing is scheduled this late,
/// so a message gets here only if the queue fell behind. It is a constant and not a setting,
/// because the number is Telegram's and our own value would change nothing.
const MAX_AGE: Duration = Duration::from_hours(48);

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

/// Acts on the messages of one run and writes down what became of them.
struct Cleaner<'a> {
    bot: &'a Throttle<Bot>,
    repos: &'a Repositories,
    cache: &'a Cache,
    cleanup: &'a CleanupPolicy,
    config: &'a SelfDestructionConfig,
    bot_admin_ttl: Duration,
}

/// Takes one batch of messages whose time has come and acts on each of them.
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn run_pending_deletions(
    bot: &Throttle<Bot>,
    repos: &Repositories,
    cache: &Cache,
    cleanup: &CleanupPolicy,
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
    let cleaner = &Cleaner { bot, repos, cache, cleanup, config, bot_admin_ttl };
    stream::iter(due)
        .for_each_concurrent(usize::from(config.concurrency), |deletion| cleaner.act_and_record(deletion))
        .await;
    Ok(())
}

/// Finishes every chat message older than [`MAX_AGE`] as expired, in batches, without claiming any.
///
/// Run once, before the worker's first tick: only a worker that isn't running can let a message
/// reach that age, and a restart is what ends that. Before the claim, because `claim_due` orders by
/// `fire_after` and would hand out the dead rows ahead of every live one. The per-message check in
/// [`Cleaner::act`] stays behind it for the row that crosses the line while the worker is up.
pub(super) async fn expire_stale(repos: &Repositories, config: &SelfDestructionConfig) {
    loop {
        let Ok(expired) = repos.deletions.expire_stale(Utc::now() - MAX_AGE, config.expiry_batch_size).await else {
            tracing::warn!("couldn't expire the stale self-destructions");
            return
        };
        if expired.is_empty() {
            return
        }
        for batch in expired {
            metrics::SELF_DESTRUCTION.record_many(batch.group, batch.kind, DeletionState::Expired, batch.count.value());
            tracing::warn!(group = %batch.group, kind = %batch.kind, count = batch.count.value(), oldest = %batch.oldest,
                "messages got too old to be deleted while they waited");
        }

        // Nothing waits on this: the rest costs a later first tick and nothing else.
        if !config.poll_interval.is_zero() {
            tokio::time::sleep(config.poll_interval).await;
        }
    }
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

impl Cleaner<'_> {
    /// Acts on one message and writes down what became of it.
    async fn act_and_record(&self, deletion: ScheduledDeletion) {
        let config = self.config;
        let id = deletion.id;
        let group = deletion.group;
        let kind = deletion.kind;
        let failures = deletion.attempts;
        let outcome = self.act(deletion).await;
        tracing::debug!(id = %id, group = %group, kind = %kind, ?outcome, "the message is dealt with");

        // Only an ending is counted, and each one only once, so the outcomes add up to the number of
        // messages. A warning and a retry are steps, not endings; retries have their own counter.
        let finish_as = |state| self.finish(id, group, kind, state);
        let result = match outcome {
            Outcome::Removed => finish_as(DeletionState::Removed).await,
            Outcome::RemovedBefore => finish_as(DeletionState::RemovedBefore).await,
            Outcome::Expired => finish_as(DeletionState::Expired).await,
            Outcome::Failed => finish_as(DeletionState::Failed).await,
            Outcome::Warned => self.repos.deletions.mark_warned(id, Utc::now() + config.warning).await,
            Outcome::Retry => {
                metrics::SELF_DESTRUCTION_RETRIES.record(group, kind);
                let next_attempt = Utc::now() + backoff(config.retry_delay, failures, config.max_retry_delay);
                match self.repos.deletions.postpone(id, next_attempt).await {
                    Ok(attempts) if attempts >= config.max_attempts => {
                        tracing::warn!(id = %id, attempts = %attempts, "giving up on a self-destructing message");
                        finish_as(DeletionState::Failed).await
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
        &self,
        id: ScheduledDeletionId,
        group: MessageGroup,
        kind: MessageKind,
        state: DeletionState,
    ) -> anyhow::Result<()> {
        metrics::SELF_DESTRUCTION.record(group, kind, state);
        self.repos.deletions.finish(id, state).await
    }

    /// Does to the message whatever its row asks for, and says what became of it.
    async fn act(&self, deletion: ScheduledDeletion) -> Outcome {
        // Telegram refuses to delete a message older than 48 hours, so the row is kept as `expired`
        // rather than spent on a request that is certain to be refused. `expire_stale` finishes nearly
        // all of them before the claim; only a row that crossed the line since, or one it failed to
        // reach, gets here.
        let too_old = (Utc::now() - deletion.created_at).to_std().unwrap_or(Duration::ZERO) > MAX_AGE;
        // A command belongs to its sender, so it can't be edited into anything — it is only ever
        // deleted, at the moment its answer's warning runs out.
        let warn = !self.config.warning.is_zero()
            && deletion.state == DeletionState::Created
            && deletion.kind != MessageKind::Command;

        match &deletion.target {
            DeletionTarget::InlineMessage(inline_message_id) => {
                let resolved = resolve_inline(inline_message_id);
                let deletable = match resolved {
                    Some((chat, _)) => rights::may_delete_inline(self.cache, chat).await,
                    None => false,
                };
                let maybe_chat = resolved.map(|(chat, _)| chat);

                let Some((chat, message)) = resolved.filter(|_| deletable && !too_old) else {
                    let otherwise = if too_old {
                        tracing::debug!(kind = %deletion.kind, created_at = %deletion.created_at,
                            "the inline message got too old to be deleted while it waited");
                        Outcome::Expired
                    } else {
                        Outcome::Failed
                    };
                    if !self.may_shrink(&deletion, maybe_chat).await {
                        return otherwise
                    }
                    if warn {
                        return self.warn_inline(&deletion, inline_message_id).await
                    }
                    return self.shrink(&deletion, inline_message_id).await
                };

                if warn {
                    return self.warn_inline(&deletion, inline_message_id).await
                }
                let result = self.bot.delete_message(chat, message).await.map(|_| ());
                if let Some(outcome) = outcome_of_inline_deletion(result) {
                    return outcome
                }
                rights::remember_deletion_right(self.cache, chat, false, self.bot_admin_ttl).await;
                if !self.may_shrink(&deletion, Some(chat)).await {
                    return Outcome::Failed
                }
                self.shrink(&deletion, inline_message_id).await
            },
            DeletionTarget::ChatMessage { chat_id, message_id } => {
                if too_old {
                    tracing::debug!(kind = %deletion.kind, created_at = %deletion.created_at,
                        "the message got too old to be deleted while it waited");
                    return Outcome::Expired
                }
                let chat = ChatId::from(*chat_id);
                let message = MessageId::from(*message_id);
                if warn {
                    let request = self.bot.edit_message_text(chat, message, self.warning(&deletion)).parse_mode(Html);
                    return outcome_of_warning(request.await.map(|_| ()), deletion.id)
                }
                self.outcome_of(self.bot.delete_message(chat, message).await.map(|_| ()), &deletion).await
            },
        }
    }

    /// Whether the chat lets an inline message the bot can't delete be replaced with the placeholder.
    async fn may_shrink(&self, deletion: &ScheduledDeletion, chat: Option<ChatId>) -> bool {
        let settings = match chat {
            Some(chat) => self.cleanup.settings(&chat.into()).await,
            None => ChatCleanupSettings::default(),
        };
        let allowed = self.config.inline_delay_for_chat(deletion.group, &settings).is_some();
        if !allowed {
            tracing::debug!(kind = %deletion.kind, "the inline message may be neither deleted nor shrunk");
        }
        allowed
    }

    async fn warn_inline(&self, deletion: &ScheduledDeletion, inline_message_id: &InlineMessageId) -> Outcome {
        let request = self.bot.edit_message_text_inline(inline_message_id.value(), self.warning(deletion))
            .parse_mode(Html);
        outcome_of_warning(request.await.map(|_| ()), deletion.id)
    }

    /// Replaces an inline message with the placeholder.
    async fn shrink(&self, deletion: &ScheduledDeletion, inline_message_id: &InlineMessageId) -> Outcome {
        let placeholder = t!("self_destruction.placeholder", locale = &deletion.lang_code);
        let request = self.bot.edit_message_text_inline(inline_message_id.value(), placeholder)
            .parse_mode(Html);
        self.outcome_of(request.await.map(|_| ()), deletion).await
    }

    fn warning(&self, deletion: &ScheduledDeletion) -> String {
        let seconds = self.config.warning.as_secs();
        t!("self_destruction.warning", locale = &deletion.lang_code, seconds = seconds).to_string()
    }


    /// Turns the answer of the Bot API into an outcome, remembering what it says about the chat.
    async fn outcome_of(&self, result: Result<(), RequestError>, deletion: &ScheduledDeletion) -> Outcome {
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
                rights::remember_deletion_right(self.cache, ChatId::from(chat_id), false, self.bot_admin_ttl).await;
            }
        }
        if is_final(&error) {
            tracing::warn!(kind = %deletion.kind, error = %error, "the message can't be removed at all");
            return Outcome::Failed
        }

        tracing::warn!(kind = %deletion.kind, error = %error, "couldn't remove the self-destructing message");
        Outcome::Retry
    }
}

/// The chat and the message an inline message was posted as, when its id names them: only a
/// supergroup's does.
fn resolve_inline(inline_message_id: &InlineMessageId) -> Option<(ChatId, MessageId)> {
    let info = resolve_inline_message_id(inline_message_id.value())
        .inspect_err(|e| tracing::warn!(error = %e, "couldn't resolve the inline_message_id"))
        .ok()?;
    info.chat_id.map(|chat_id| (ChatId::from(chat_id), info.message_id))
}

/// What the deletion of an inline message means for the row, or `None` when the bot can't delete
/// it and the message is to be shrunk instead.
fn outcome_of_inline_deletion(result: Result<(), RequestError>) -> Option<Outcome> {
    let Err(error) = result else {
        return Some(Outcome::Removed)
    };
    if is_gone(&error) {
        return Some(Outcome::RemovedBefore)
    }
    if is_final(&error) {
        tracing::debug!(error = %error, "the inline message can't be deleted, so it is shrunk instead");
        return None
    }
    tracing::warn!(error = %error, "couldn't delete the self-destructing inline message");
    Some(Outcome::Retry)
}

/// What the warning attempt means for the row.
///
/// A message that is already gone will not come back, so the row ends here rather than a grace
/// period and a certain-to-fail deletion later. Any other failure carries on: a message that could
/// not be warned is still worth deleting on time, and the notice is the part worth losing.
fn outcome_of_warning(result: Result<(), RequestError>, id: ScheduledDeletionId) -> Outcome {
    let error = match result {
        Ok(()) => return Outcome::Warned,
        Err(e) => e,
    };
    if is_gone(&error) {
        tracing::debug!(id = %id, "the message was gone before its warning");
        return Outcome::RemovedBefore
    }
    tracing::warn!(id = %id, error = %error, "couldn't edit the message with the warning");
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
    use serde_json::json;
    use crate::config::{CacheConfig, CacheMode, DeletionMode, InlineGroups};
    use crate::domain::primitives::{AttemptsCount, LanguageCode};
    use crate::fake_telegram::FakeApi;
    use crate::handlers::utils::inline_message_id_of;
    use crate::repo::test::{fresh_db, repos};
    use super::*;

    const CHAT_ID: i64 = -1001100294568;
    /// Too short to be any of Telegram's formats, so it names no chat.
    const UNRESOLVABLE_ID: &str = "AgAAAOEcAABzXwsRJ0Cs2A";

    /// A message someone else removed used to be warned first and only found missing a grace
    /// period later, at the cost of two requests and a warn-level line for a normal event.
    #[test]
    fn a_message_gone_before_its_warning_ends_there() {
        let id = ScheduledDeletionId::new(1);
        for gone in [ApiError::MessageToEditNotFound, ApiError::MessageToDeleteNotFound, ApiError::MessageIdInvalid] {
            let outcome = outcome_of_warning(Err(RequestError::Api(gone)), id);
            assert_eq!(outcome, Outcome::RemovedBefore);
        }
    }

    /// Anything else is transient as far as the warning is concerned: the notice is lost, the
    /// deletion is not.
    #[test]
    fn a_warning_that_merely_failed_still_leads_to_a_deletion() {
        let id = ScheduledDeletionId::new(1);
        assert_eq!(outcome_of_warning(Ok(()), id), Outcome::Warned);

        let transient = RequestError::Api(ApiError::Unknown("Bad Gateway".to_owned()));
        assert_eq!(outcome_of_warning(Err(transient), id), Outcome::Warned);

        let rate_limited = RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(5));
        assert_eq!(outcome_of_warning(Err(rate_limited), id), Outcome::Warned);
    }

    /// Only a refusal hands the message over to the placeholder; a transient failure is retried.
    #[test]
    fn only_a_refused_inline_deletion_falls_back() {
        assert_eq!(outcome_of_inline_deletion(Ok(())), Some(Outcome::Removed));

        let gone = RequestError::Api(ApiError::MessageToDeleteNotFound);
        assert_eq!(outcome_of_inline_deletion(Err(gone)), Some(Outcome::RemovedBefore));

        for refused in [ApiError::MessageCantBeDeleted, ApiError::ChatNotFound, ApiError::BotKickedFromSupergroup] {
            assert_eq!(outcome_of_inline_deletion(Err(RequestError::Api(refused))), None);
        }

        let transient = RequestError::Api(ApiError::Unknown("Bad Gateway".to_owned()));
        assert_eq!(outcome_of_inline_deletion(Err(transient)), Some(Outcome::Retry));
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

    /// An inline message the bot may delete is warned first, like any other.
    #[tokio::test]
    async fn an_inline_message_is_warned_before_it_is_deleted() {
        let (outcome, api, _) = act_on_inline(inline_message_id_of(CHAT_ID), "", DeletionState::Created).await;

        assert_eq!(outcome, Outcome::Warned);
        assert_eq!(api.methods(), ["EditMessageText"]);
        assert_eq!(api.calls()[0].1["text"], warning_text());
    }

    /// The deletion is refused, so the message is edited into the placeholder, and the bot
    /// remembers that it may not delete messages there.
    #[tokio::test]
    async fn an_inline_message_that_cant_be_deleted_is_shrunk_instead() {
        let (outcome, api, cache) = act_on_inline(inline_message_id_of(CHAT_ID), "report", DeletionState::Warned).await;

        assert_eq!(outcome, Outcome::Removed);
        assert_eq!(api.methods(), ["DeleteMessage", "EditMessageText"]);
        let calls = api.calls();
        assert_eq!(calls[0].1["chat_id"], CHAT_ID);
        assert_eq!(calls[0].1["message_id"], 42);
        assert_eq!(calls[1].1["inline_message_id"], inline_message_id_of(CHAT_ID));
        assert_eq!(calls[1].1["text"], placeholder_text());
        let known = rights::cached_deletion_right(&cache, ChatId(CHAT_ID)).await;
        assert_eq!(known, Some(false));
    }

    /// A group the placeholder is not allowed for keeps its message when the deletion is refused.
    #[tokio::test]
    async fn an_inline_message_that_may_not_be_shrunk_is_left_alone() {
        let (outcome, api, _) = act_on_inline(inline_message_id_of(CHAT_ID), "", DeletionState::Warned).await;

        assert_eq!(outcome, Outcome::Failed);
        assert_eq!(api.methods(), ["DeleteMessage"]);
    }

    /// An id that names no chat can only be shrunk, and it is warned about first all the same.
    #[tokio::test]
    async fn an_inline_message_is_warned_before_it_is_shrunk() {
        let (outcome, api, _) = act_on_inline(UNRESOLVABLE_ID.to_owned(), "report", DeletionState::Created).await;
        assert_eq!(outcome, Outcome::Warned);
        assert_eq!(api.methods(), ["EditMessageText"]);
        assert_eq!(api.calls()[0].1["text"], warning_text());

        let (outcome, api, _) = act_on_inline(UNRESOLVABLE_ID.to_owned(), "report", DeletionState::Warned).await;
        assert_eq!(outcome, Outcome::Removed);
        assert_eq!(api.methods(), ["EditMessageText"]);
        assert_eq!(api.calls()[0].1["text"], placeholder_text());
    }

    /// A message that will be neither deleted nor shrunk isn't warned about either.
    #[tokio::test]
    async fn an_inline_message_that_will_stay_is_not_warned_about() {
        let (outcome, api, _) = act_on_inline(UNRESOLVABLE_ID.to_owned(), "", DeletionState::Created).await;

        assert_eq!(outcome, Outcome::Failed);
        assert!(api.methods().is_empty());
    }

    /// Runs one inline `Report` message through [`Cleaner::act`] against an API that refuses every
    /// deletion, with `inline_groups` deciding whether a placeholder is allowed.
    async fn act_on_inline(
        inline_message_id: String,
        inline_groups: &str,
        state: DeletionState,
    ) -> (Outcome, FakeApi, Cache) {
        let api = FakeApi::start(|method| match method {
            "DeleteMessage" => json!({
                "ok": false,
                "error_code": 400,
                "description": "Bad Request: message can't be deleted"
            }),
            _ => json!({ "ok": true, "result": true }),
        }).await;
        let db = fresh_db().await;
        let repos = repos(&db);
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let cleanup = CleanupPolicy::new(repos.chats.clone(), cache.clone(), Duration::from_hours(1));
        let config = SelfDestructionConfig {
            mode: DeletionMode::Enabled,
            report: Duration::from_mins(2),
            warning: Duration::from_secs(15),
            inline_groups: inline_groups.parse::<InlineGroups>().expect("couldn't parse the groups"),
            ..Default::default()
        };
        let bot = Throttle::new_spawn(api.bot.clone(), Default::default());
        let cleaner = Cleaner {
            bot: &bot, repos: &repos, cache: &cache, cleanup: &cleanup, config: &config,
            bot_admin_ttl: Duration::from_hours(1),
        };

        let outcome = cleaner.act(inline_deletion(inline_message_id, state)).await;
        (outcome, api, cache)
    }

    fn inline_deletion(inline_message_id: String, state: DeletionState) -> ScheduledDeletion {
        ScheduledDeletion {
            id: ScheduledDeletionId::new(1),
            target: DeletionTarget::InlineMessage(InlineMessageId::new(inline_message_id)),
            kind: MessageKind::Inline,
            group: MessageGroup::Report,
            lang_code: LanguageCode::new("en".to_owned()),
            state,
            created_at: Utc::now(),
            attempts: AttemptsCount::new(0),
        }
    }

    fn warning_text() -> String {
        t!("self_destruction.warning", locale = "en", seconds = 15).to_string()
    }

    fn placeholder_text() -> String {
        t!("self_destruction.placeholder", locale = "en").to_string()
    }
}
