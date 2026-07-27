use std::collections::HashMap;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::payloads::SendMessageSetters;
use teloxide::requests::Requester;
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{ChatId, UserId as TeloxideUserId};
use teloxide::types::ParseMode::Html;
use crate::config::AppConfig;
use crate::domain::primitives::{LanguageCode, SupportedLanguage};
use crate::domain::primitives::chat::{ChatIdKind, TelegramChatId};
use crate::repo::{Repositories, ShrinkEvent};
use crate::users::LanguageService;

/// Runs the daily shrink: applies the decay in one DB statement, then broadcasts a per-chat summary
/// to every affected group chat. Inline-only chats (no messageable `chat_id`) are silently skipped —
/// their members see the events via the `shrinks` inline command instead.
pub async fn run_daily_shrink(
    bot: Bot,
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

    let mut by_chat: HashMap<TelegramChatId, Vec<ShrinkEvent>> = HashMap::new();
    for event in events {
        if let Some(chat_id) = event.messageable_chat_id {
            by_chat.entry(chat_id).or_default().push(event);
        }
    }

    for (chat_id, victims) in by_chat {
        if let Err(e) = broadcast_shrink(&bot, &repos, &language_service, &config, chat_id, &victims).await {
            log::warn!("daily shrink: couldn't notify chat {chat_id}: {e:#}");
        }
    }
    Ok(())
}

async fn broadcast_shrink(
    bot: &Bot,
    repos: &Repositories,
    language_service: &LanguageService,
    config: &AppConfig,
    chat_id: TelegramChatId,
    victims: &[ShrinkEvent],
) -> anyhow::Result<()> {
    let chat = ChatIdKind::from(chat_id);
    let lang = resolve_broadcast_language(repos, language_service, config, &chat).await;
    let lang_code = LanguageCode::new(lang.to_string());

    let lines = victims.iter()
        .map(|v| {
            let name = v.owner_name.escaped();
            t!("commands.shrink.notification.line", locale = &lang_code,
                uid = v.uid, name = name, lost = v.lost_length, length = v.new_length)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let header = t!("commands.shrink.notification.header", locale = &lang_code);
    let text = format!("{header}\n{lines}");

    bot.send_message(ChatId(chat_id.value()), text)
        .parse_mode(Html)
        .disable_link_preview(true)
        .await?;
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
