use std::collections::HashMap;
use chrono::Utc;
use teloxide::Bot;
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
use crate::repo::{Repositories, ShrinkEvent};
use crate::users::LanguageService;

/// Runs the daily shrink: applies the decay in one DB statement, then broadcasts a per-chat summary
/// to every affected group chat. Inline-only chats (no messageable `chat_id`) are silently skipped —
/// their members see the events via the `shrinks` inline command instead.
pub async fn run_daily_shrink(
    bot: Throttle<Bot>,
    repos: Repositories,
    language_service: LanguageService,
    config: AppConfig,
) -> anyhow::Result<()> {
    let events = repos.shrinks
        .perform_daily_shrink(
            config.stale_dicks_shrinking.ratio,
            config.stale_dicks_shrinking.grace_period_days,
            config.stale_dicks_shrinking.ramp_up_days,
        )
        .await?;
    if events.is_empty() {
        log::info!("daily shrink: nothing to shrink today");
        return Ok(());
    }

    // The scheduler only ever wakes up at UTC midnight (see `spawn_daily_shrink`), and the shrinks
    // just logged carry Postgres's `current_date` — also UTC — so this is the exact day they belong
    // to. Captured once so every chat's broadcast pins the same date its shrinks were logged under.
    let today = Utc::now().date_naive();

    let mut by_chat: HashMap<TelegramChatId, Vec<ShrinkEvent>> = HashMap::new();
    for event in events {
        if let Some(chat_id) = event.messageable_chat_id {
            by_chat.entry(chat_id).or_default().push(event);
        }
    }

    for (chat_id, victims) in by_chat {
        if let Err(e) = broadcast_shrink(&bot, &repos, &language_service, &config, chat_id, today, &victims).await {
            log::warn!("daily shrink: couldn't notify chat {chat_id}: {e:#}");
        }
    }
    Ok(())
}

/// Sends page 0 of the chat's shrink list, rendered straight from `victims` — no query, since the
/// daily job already holds exactly this data (and for most chats it's the whole list anyway).
/// Pages 1+ (only reachable by an actual button press) fall back to [`shrinks_page_impl`], pinned
/// to `date` so a click on day D+1 still shows day D's shrinks.
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
