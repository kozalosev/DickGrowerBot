use std::time::Duration;
use autometrics::autometrics;
use chrono::Utc;
use futures::{stream, StreamExt};
use teloxide::{ApiError, Bot, RequestError};
use teloxide::adaptors::Throttle;
use teloxide::payloads::SendMessageSetters;
use teloxide::requests::Requester;
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{ChatId, ReplyMarkup, UserId as TeloxideUserId};
use teloxide::types::ParseMode::Html;
use domain_types::traits::ApproxInto;
use crate::config::AppConfig;
use crate::domain::primitives::{LanguageCode, Page, ScheduledBroadcastId, SupportedLanguage};
use crate::domain::primitives::chat::ChatIdKind;
use crate::handlers::shrink::{build_shrink_keyboard, shrinks_page_impl, ShrinkView};
use crate::metrics;
use crate::repo::{BroadcastState, Repositories, ScheduledBroadcast};
use super::backoff;
use crate::topics::TopicPolicy;
use crate::users::LanguageService;

/// The services every summary needs, bundled so the per-chat calls stay readable.
#[derive(Clone, Copy)]
pub struct BroadcastDeps<'a> {
    pub bot: &'a Throttle<Bot>,
    pub repos: &'a Repositories,
    pub language_service: &'a LanguageService,
    pub topics: &'a TopicPolicy,
    pub config: &'a AppConfig,
}

/// What the worker decided to do with a row once it had tried to send its summary.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The chat got its summary.
    Sent,
    /// Something transient went wrong; the row is tried again later.
    Retry,
    /// The summary sat in the queue until it stopped being worth sending.
    Expired,
    /// The bot can't post to that chat at all, which marks the chat too.
    Unreachable,
    /// It won't work, now or later, for a reason that says nothing about the chat.
    Failed,
}

/// Takes one batch of summaries whose time has come and sends each of them.
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn run_pending_broadcasts(deps: BroadcastDeps<'_>) -> anyhow::Result<()> {
    let config = &deps.config.daily_shrink.broadcast;
    let due = deps.repos.broadcasts
        .claim_due(config.batch_size, Utc::now() + config.lease)
        .await?;
    // The empty runs are measured too: an idle worker is what tells a queue that keeps up from one
    // that is merely being asked for less than it holds.
    metrics::DAILY_SHRINK_BROADCAST_BATCH_SIZE.observe(due.len().approx_into());
    if due.is_empty() {
        return Ok(())
    }
    tracing::debug!(count = due.len(), "sending the shrink summaries that are due");

    // Concurrently, because what one run gets through would otherwise be one chat per round trip to
    // Telegram however large the batch — which is what let a broadcast to two hundred thousand
    // chats outlast the day it belonged to. The rate is still Telegram's to set: every request here
    // goes through the shared `Throttle`.
    stream::iter(due)
        .for_each_concurrent(usize::from(config.concurrency), |broadcast| async move {
            send_and_record(deps, broadcast).await
        })
        .await;
    Ok(())
}

/// Removes the rows that were finished long enough ago.
///
/// Separate from the worker on purpose: the finished rows are the only account of what the worker
/// did, so how long they are kept is a decision of its own, and clearing them must never be part of
/// the run that produced them.
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn clean_finished_broadcasts(repos: &Repositories, retention: Duration) -> anyhow::Result<()> {
    let older_than = Utc::now() - retention;
    let removed = repos.broadcasts.delete_finished(older_than).await?;
    if removed > 0 {
        tracing::info!(removed, "cleaned the finished shrink summaries up");
    }
    Ok(())
}

/// Sends one summary and writes down what became of it.
#[tracing::instrument(skip_all, fields(id = %broadcast.id, chat_id = %broadcast.chat_id, date = %broadcast.shrink_date))]
async fn send_and_record(deps: BroadcastDeps<'_>, broadcast: ScheduledBroadcast) {
    let config = &deps.config.daily_shrink.broadcast;
    let id = broadcast.id;
    let failures = broadcast.attempts;
    let outcome = send(deps, &broadcast).await;

    // Only an ending is counted, and each one only once, so the outcomes add up to the number of
    // summaries. A retry is a step, not an ending, and has a counter of its own.
    let result = match outcome {
        Outcome::Sent => finish(deps.repos, id, BroadcastState::Sent).await,
        Outcome::Expired => finish(deps.repos, id, BroadcastState::Expired).await,
        Outcome::Unreachable => finish(deps.repos, id, BroadcastState::Unreachable).await,
        Outcome::Failed => finish(deps.repos, id, BroadcastState::Failed).await,
        Outcome::Retry => {
            metrics::DAILY_SHRINK.broadcast_retried();
            let next_attempt = Utc::now() + backoff(config.retry_delay, failures, config.max_retry_delay);
            match deps.repos.broadcasts.postpone(id, next_attempt).await {
                Ok(attempts) if attempts >= config.max_attempts => {
                    tracing::warn!(attempts = %attempts, "giving up on a shrink summary");
                    finish(deps.repos, id, BroadcastState::Failed).await
                },
                other => other.map(|_| ()),
            }
        },
    };
    if let Err(e) = result {
        tracing::error!(error = format!("{e:#}"), "couldn't record the outcome of a shrink summary");
    }
}

/// Stores the state a row ended in and counts that ending. Both happen here, so the table and the
/// counter always say the same thing.
async fn finish(
    repos: &Repositories,
    id: ScheduledBroadcastId,
    state: BroadcastState,
) -> anyhow::Result<()> {
    metrics::DAILY_SHRINK.broadcast_finished(state);
    repos.broadcasts.finish(id, state).await
}

/// Sends page 0 of the chat's shrink list for the day the row names, and says what became of it.
///
/// The page comes from the same query the "next page" button uses, so what a chat reads first and
/// what it reads after tapping are one list rather than two orderings of it.
async fn send(deps: BroadcastDeps<'_>, broadcast: &ScheduledBroadcast) -> Outcome {
    let BroadcastDeps { bot, repos, topics, config, .. } = deps;
    let broadcast_config = &config.daily_shrink.broadcast;

    // A summary that waited this long has stopped being news, and the chat has the `shrinks`
    // command for the history. Only a queue that fell behind can bring one here.
    let age = (Utc::now() - broadcast.created_at).to_std().unwrap_or(Duration::ZERO);
    if age > broadcast_config.max_age {
        tracing::warn!(created_at = %broadcast.created_at, "the shrink summary got too old to be worth sending");
        return Outcome::Expired
    }

    let chat = ChatIdKind::from(broadcast.chat_id);
    let lang = resolve_broadcast_language(deps, &chat).await;
    let lang_code = LanguageCode::new(lang.to_string());

    let page = match shrinks_page_impl(repos, config, &chat, &lang_code,
                                       ShrinkView::Broadcast, broadcast.shrink_date, Page::first()).await {
        Ok(page) => page,
        Err(e) => {
            tracing::warn!(error = format!("{e:#}"), "couldn't render the shrink summary");
            return Outcome::Retry
        },
    };
    // A single day by definition, so day-navigation (`adjacent`) is always `None`.
    let keyboard = build_shrink_keyboard(ShrinkView::Broadcast, broadcast.shrink_date,
                                         Page::first(), page.has_more_pages, None);

    // The throttled request wraps the payload, so the keyboard goes through the setter rather than
    // the field the plain `Bot` exposes.
    let mut request = bot.send_message(ChatId(broadcast.chat_id.value()), page.lines)
        .parse_mode(Html)
        .disable_link_preview(true);
    if let Some(keyboard) = keyboard {
        request = request.reply_markup(ReplyMarkup::InlineKeyboard(keyboard));
    }
    // Nothing is being replied to here, so the topic has to be named outright. Left to itself the
    // message would go to General — which a forum that keeps the bot elsewhere may well have
    // closed, and posting into a closed topic is refused.
    if let Some(topic) = topics.allowed(&chat).await.primary() {
        request = request.message_thread_id(topic.into());
    }

    outcome_of(request.await.map(|_| ()), repos, broadcast).await
}

/// Turns the answer of the Bot API into an outcome, remembering what it says about the chat.
async fn outcome_of(
    result: Result<(), RequestError>,
    repos: &Repositories,
    broadcast: &ScheduledBroadcast,
) -> Outcome {
    let error = match result {
        Ok(()) => return Outcome::Sent,
        Err(e) => e,
    };

    if !is_chat_unreachable(&error) {
        if is_final(&error) {
            tracing::warn!(error = %error, "the shrink summary can't be sent to this chat at all");
            return Outcome::Failed
        }
        tracing::warn!(error = %error, "couldn't notify the chat about the shrinks");
        return Outcome::Retry
    }

    // Marking is best-effort: a chat that stays unmarked is merely queued again tomorrow.
    tracing::info!(error = %error, "the chat is unreachable, skipping it from now on");
    repos.chats.mark_unreachable(&broadcast.chat_id)
        .await
        .unwrap_or_else(|e| tracing::warn!(error = format!("{e:#}"), "couldn't mark the chat as unreachable"));
    Outcome::Unreachable
}

/// The three errors teloxide has no variant for, in the wording Telegram actually sends. The first
/// two are what a modern group returns instead of [`ApiError::BotKicked`]; the last one is a bot
/// that is still a member but was muted by an admin.
const UNREACHABLE_ERROR_TEXTS: [&str; 3] = [
    "bot was kicked from the group chat",
    "bot is not a member of the group chat",
    "have no rights to send a message",
];

/// Whether a failed send means the bot can't post to that chat at all, as opposed to a hiccup worth
/// retrying.
///
/// Rate limits, timeouts and network errors are all transient, so they never mark a chat. Neither
/// does [`RequestError::MigrateToChatId`]: the chat is alive and well under its new id, and the
/// `migration_handler` repoints its row on the service message Telegram sends alongside.
fn is_chat_unreachable(error: &RequestError) -> bool {
    let RequestError::Api(api_err) = error else {
        return false
    };
    match api_err {
        ApiError::BotBlocked
        | ApiError::BotKicked
        | ApiError::BotKickedFromSupergroup
        | ApiError::BotKickedFromChannel
        | ApiError::ChatNotFound
        | ApiError::GroupDeactivated
        | ApiError::UserDeactivated
        | ApiError::NotEnoughRightsToPostMessages => true,
        // Telegram keeps adding wordings teloxide doesn't know yet, and the ones a group hits most
        // often are among them, so the raw text is the only thing left to look at.
        ApiError::Unknown(text) => {
            let text = text.to_lowercase();
            UNREACHABLE_ERROR_TEXTS.iter().any(|known| text.contains(known))
        }
        _ => false
    }
}

/// Whether retrying could ever help. A rejection teloxide has a variant for is one Telegram thought
/// about and refused, so the same payload gets the same answer, and spending three attempts on it
/// costs three requests per chat — which at a few hundred thousand chats is the difference between
/// a hiccup and an outage. `Unknown` stays retryable: Telegram's own 5xx answers arrive that way.
fn is_final(error: &RequestError) -> bool {
    matches!(error, RequestError::Api(api) if !matches!(api, ApiError::Unknown(_)))
}

/// Picks the language for a chat's summary: the chat-wide override wins; otherwise, when the
/// `getMany` toggle is on, the most popular language among the chat's players; English otherwise.
#[tracing::instrument(skip_all)]
async fn resolve_broadcast_language(deps: BroadcastDeps<'_>, chat: &ChatIdKind) -> SupportedLanguage {
    let BroadcastDeps { repos, language_service, config, .. } = deps;
    match repos.chats.get_chat_language(chat).await {
        Ok(Some(lang)) => {
            metrics::BROADCAST_LANGUAGE.decided_by_chat();
            return lang
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't read the language of the chat"),
    }

    if config.features.most_popular_language_enabled {
        let uids: Vec<TeloxideUserId> = repos.dicks.get_player_uids(chat).await
            .inspect_err(|e| tracing::warn!(error = format!("{e:#}"), "couldn't list the players of the chat"))
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        if let Some(lang) = language_service.popular_language(&uids).await {
            metrics::BROADCAST_LANGUAGE.decided_by_tally();
            return lang;
        }
    }
    metrics::BROADCAST_LANGUAGE.defaulted();
    SupportedLanguage::EN
}

#[cfg(test)]
mod tests {
    use teloxide::{ApiError, RequestError};
    use super::is_chat_unreachable;

    #[test]
    fn known_api_errors_make_a_chat_unreachable() {
        for err in [
            ApiError::BotBlocked,
            ApiError::BotKicked,
            ApiError::BotKickedFromSupergroup,
            ApiError::BotKickedFromChannel,
            ApiError::ChatNotFound,
            ApiError::GroupDeactivated,
            ApiError::UserDeactivated,
            ApiError::NotEnoughRightsToPostMessages,
        ] {
            assert!(is_chat_unreachable(&RequestError::Api(err.clone())), "{err:?} should mark the chat");
        }
    }

    /// The wordings a group really gets: teloxide has no variant for any of them, so they arrive as
    /// `Unknown` and only the text tells them apart from a transient error.
    #[test]
    fn unknown_api_errors_are_matched_by_their_text() {
        for text in [
            "Forbidden: bot was kicked from the group chat",
            "Forbidden: bot is not a member of the group chat",
            "Bad Request: have no rights to send a message",
        ] {
            let err = RequestError::Api(ApiError::Unknown(text.to_owned()));
            assert!(is_chat_unreachable(&err), "{text:?} should mark the chat");
        }
    }

    #[test]
    fn transient_errors_leave_the_chat_alone() {
        let unknown = RequestError::Api(ApiError::Unknown("Bad Request: message is too long".to_owned()));
        assert!(!is_chat_unreachable(&unknown));

        let too_long = RequestError::Api(ApiError::MessageIsTooLong);
        assert!(!is_chat_unreachable(&too_long));

        let retry_after = RequestError::RetryAfter(teloxide::types::Seconds::from_seconds(30));
        assert!(!is_chat_unreachable(&retry_after));

        // A migrated chat is alive under its new id; the migration handler repoints its row.
        let migrated = RequestError::MigrateToChatId(teloxide::types::ChatId(-100));
        assert!(!is_chat_unreachable(&migrated));
    }
}
