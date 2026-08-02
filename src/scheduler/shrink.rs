use std::collections::HashMap;
use autometrics::autometrics;
use chrono::Utc;
use teloxide::{ApiError, Bot, RequestError};
use teloxide::adaptors::Throttle;
use teloxide::payloads::SendMessageSetters;
use teloxide::requests::Requester;
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{ChatId, ReplyMarkup, UserId as TeloxideUserId};
use teloxide::types::ParseMode::Html;
use crate::config::AppConfig;
use crate::domain::primitives::{LanguageCode, Page, SupportedLanguage};
use crate::domain::primitives::chat::{ChatIdKind, TelegramChatId};
use crate::handlers::shrink::{build_shrink_keyboard, render_shrinks_page, ShrinkView};
use crate::metrics;
use crate::repo::{Repositories, ShrinkEvent};
use crate::users::LanguageService;

/// Runs the daily shrink: applies the decay in one DB statement, then broadcasts a per-chat summary
/// to every affected group chat. Inline-only chats (no messageable `chat_id`) get no message. Their
/// members see the events via the `shrinks` inline command. Their victims are still counted, under
/// the `inline_only` label of [`metrics::DAILY_SHRINK`].
#[autometrics]
#[tracing::instrument(skip_all)]
pub async fn run_daily_shrink(
    bot: Throttle<Bot>,
    repos: Repositories,
    language_service: LanguageService,
    config: AppConfig,
) -> anyhow::Result<()> {
    let events = repos.shrinks
        .perform_daily_shrink(
            config.daily_shrink.ratio,
            config.daily_shrink.inactivity_days,
            config.daily_shrink.ramp_up_days,
        )
        .await
        .inspect_err(|_| metrics::DAILY_SHRINK.run_failed())?;
    if events.is_empty() {
        metrics::DAILY_SHRINK.run_empty();
        log::info!("daily shrink: nothing to shrink today");
        return Ok(());
    }
    metrics::DAILY_SHRINK.run_succeeded();

    // The scheduler only ever wakes up at UTC midnight (see `spawn_daily_shrink`), and the shrinks
    // just logged carry Postgres's `current_date` — also UTC — so this is the exact day they belong
    // to. Captured once so every chat's broadcast pins the same date its shrinks were logged under.
    let today = Utc::now().date_naive();

    let total = events.len() as u64;
    let mut by_chat: HashMap<TelegramChatId, Vec<ShrinkEvent>> = HashMap::new();
    // Chats the bot is known to have lost access to are dropped here rather than at the broadcast:
    // there's no point holding their events, and skipping is counted per chat, not per victim.
    let mut unreachable_victims_count_by_chat = HashMap::new();
    for event in events {
        match event.messageable_chat_id {
            Some(chat_id) if event.is_unreachable => {
                *unreachable_victims_count_by_chat.entry(chat_id).or_default() += 1;
            }
            Some(chat_id) => by_chat.entry(chat_id).or_default().push(event),
            None => {}
        }
    }
    let to_broadcast_count: u64 = by_chat.values().map(|victims| victims.len() as u64).sum();
    let unreachable_victims_count: u64 = unreachable_victims_count_by_chat.values().sum();
    metrics::DAILY_SHRINK.victims_to_broadcast(to_broadcast_count);
    metrics::DAILY_SHRINK.victims_unreachable(unreachable_victims_count);
    metrics::DAILY_SHRINK.victims_inline_only(total - to_broadcast_count - unreachable_victims_count);
    metrics::DAILY_SHRINK.broadcast_skipped(unreachable_victims_count_by_chat.len() as u64);
    if !unreachable_victims_count_by_chat.is_empty() {
        log::info!("daily shrink: skipped {} unreachable chat(s) with {unreachable_victims_count} victim(s)",
            unreachable_victims_count_by_chat.len());
    }

    for (chat_id, victims) in by_chat {
        match broadcast_shrink(&bot, &repos, &language_service, &config, chat_id, today, &victims).await {
            Ok(_) => metrics::DAILY_SHRINK.broadcast_sent(),
            Err(e) => handle_broadcast_error(&repos, chat_id, e).await,
        }
    }
    Ok(())
}

/// Logs a failed broadcast and, when Telegram says the bot can't post to that chat at all, marks
/// the chat so the next runs don't try again.
///
/// Marking is best-effort on purpose: a chat that stays unmarked is merely retried tomorrow,
/// and one broken chat must not abort the rest of the broadcast.
#[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
async fn handle_broadcast_error(repos: &Repositories, chat_id: TelegramChatId, err: anyhow::Error) {
    if !is_chat_unreachable(&err) {
        metrics::DAILY_SHRINK.broadcast_failed();
        log::warn!("daily shrink: couldn't notify chat {chat_id}: {err:#}");
        return;
    }
    metrics::DAILY_SHRINK.broadcast_unreachable();
    log::info!("daily shrink: the chat {chat_id} is unreachable, skipping it from now on: {err:#}");
    repos.chats.mark_unreachable(&chat_id)
        .await
        .unwrap_or_else(|e| log::warn!("daily shrink: couldn't mark the chat {chat_id} as unreachable: {e:#}"));
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
/// retrying tomorrow.
///
/// Rate limits, timeouts and network errors are all transient, so they never mark a chat. Neither
/// does [`RequestError::MigrateToChatId`]: the chat is alive and well under its new id, and the
/// `migration_handler` repoints its row on the service message Telegram sends alongside.
fn is_chat_unreachable(err: &anyhow::Error) -> bool {
    let Some(RequestError::Api(api_err)) = err.downcast_ref::<RequestError>() else {
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

/// Sends page 0 of the chat's shrink list, rendered straight from `victims` — no query, since the
/// daily job already holds exactly this data (and for most chats it's the whole list anyway).
/// Pages 1+ (only reachable by an actual button press) fall back to [`shrinks_page_impl`], pinned
/// to `date` so a click on day D+1 still shows day D's shrinks.
#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = %chat_id, date = %date, victims = victims.len()))]
async fn broadcast_shrink(
    bot: &Throttle<Bot>,
    repos: &Repositories,
    language_service: &LanguageService,
    config: &AppConfig,
    chat_id: TelegramChatId,
    date: chrono::NaiveDate,
    victims: &[ShrinkEvent],
) -> anyhow::Result<()> {
    let chat = ChatIdKind::from(chat_id);
    let lang = resolve_broadcast_language(repos, language_service, config, &chat).await;
    let lang_code = LanguageCode::new(lang.to_string());

    let has_more_pages = victims.len() > config.top_limit.value() as usize;
    let page = render_shrinks_page(victims, config, &lang_code, ShrinkView::Broadcast, has_more_pages);
    // A single day by definition, so day-navigation (`adjacent`) is always `None`.
    let keyboard = build_shrink_keyboard(ShrinkView::Broadcast, date, Page::first(), page.has_more_pages, None);

    // The throttled request wraps the payload, so the keyboard goes through the setter rather than
    // the field the plain `Bot` exposes.
    let mut request = bot.send_message(ChatId(chat_id.value()), page.lines)
        .parse_mode(Html)
        .disable_link_preview(true);
    if let Some(keyboard) = keyboard {
        request = request.reply_markup(ReplyMarkup::InlineKeyboard(keyboard));
    }
    request.await?;
    Ok(())
}

/// Picks the language for a chat's broadcast: the chat-wide override wins; otherwise, when the
/// `getMany` toggle is on, the most popular language among the chat's players; English otherwise.
#[tracing::instrument(skip_all, fields(chat_id = %chat))]
async fn resolve_broadcast_language(
    repos: &Repositories,
    language_service: &LanguageService,
    config: &AppConfig,
    chat: &ChatIdKind,
) -> SupportedLanguage {
    match repos.chats.get_chat_language(chat).await {
        Ok(Some(lang)) => return lang,
        Ok(None) => {}
        Err(e) => log::warn!("daily shrink: couldn't read the language of {chat}: {e:#}"),
    }

    if config.features.most_popular_language_enabled {
        let uids: Vec<TeloxideUserId> = repos.dicks.get_player_uids(chat).await
            .inspect_err(|e| log::warn!("daily shrink: couldn't list players of {chat}: {e:#}"))
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        if let Some(lang) = language_service.popular_language(&uids).await {
            return lang;
        }
    }
    SupportedLanguage::EN
}

#[cfg(test)]
mod tests {
    use teloxide::{ApiError, RequestError};
    use super::is_chat_unreachable;

    fn api_error(err: ApiError) -> anyhow::Error {
        RequestError::Api(err).into()
    }

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
            assert!(is_chat_unreachable(&api_error(err.clone())), "{err:?} should mark the chat");
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
            let err = api_error(ApiError::Unknown(text.to_owned()));
            assert!(is_chat_unreachable(&err), "{text:?} should mark the chat");
        }
    }

    #[test]
    fn transient_errors_leave_the_chat_alone() {
        let unknown = api_error(ApiError::Unknown("Bad Request: message is too long".to_owned()));
        assert!(!is_chat_unreachable(&unknown));

        let too_long = api_error(ApiError::MessageIsTooLong);
        assert!(!is_chat_unreachable(&too_long));

        let retry_after = anyhow::Error::from(RequestError::RetryAfter(
            teloxide::types::Seconds::from_seconds(30)));
        assert!(!is_chat_unreachable(&retry_after));

        // A migrated chat is alive under its new id; the migration handler repoints its row.
        let migrated = anyhow::Error::from(RequestError::MigrateToChatId(teloxide::types::ChatId(-100)));
        assert!(!is_chat_unreachable(&migrated));
    }

    /// Anything that isn't a `RequestError` at all — the broadcast can also fail before the send,
    /// e.g. while reading the chat's language.
    #[test]
    fn non_telegram_errors_leave_the_chat_alone() {
        let err = anyhow::anyhow!("couldn't render the page");
        assert!(!is_chat_unreachable(&err));
    }
}
