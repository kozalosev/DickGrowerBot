use std::fmt::Debug;
use std::str::FromStr;
use anyhow::anyhow;
use futures::TryFutureExt;
use rust_i18n::t;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString};
use teloxide::Bot;
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::requests::Requester;
use teloxide::types::*;
use teloxide::types::ParseMode::Html;
use crate::config::AppConfig;
use crate::handlers::{build_pagination_keyboard, dick, dod, ensure_lang_code, FromRefs, HandlerResult, utils};
use crate::handlers::utils::page::Page;
use crate::metrics;
use crate::repo::{ChatIdFull, NoChatIdError, ChatIdSource, Repositories};

#[derive(Debug, strum_macros::Display, EnumIter, EnumString)]
#[strum(serialize_all = "snake_case")]
enum InlineCommand {
    Grow,
    Top,
    DickOfDay,
}

struct InlineResult {
    text: String,
    keyboard: Option<InlineKeyboardMarkup>,
}

impl InlineResult {
    fn text(value: String) -> Self {
        Self {
            text: value,
            keyboard: None,
        }
    }
}

impl InlineCommand {
    async fn execute(&self, repos: &Repositories, config: AppConfig, from_refs: FromRefs<'_>) -> anyhow::Result<InlineResult> {
        match self {
            InlineCommand::Grow => {
                metrics::CMD_GROW_COUNTER.inline.inc();
                dick::grow_impl(repos, config, from_refs)
                    .await
                    .map(|res| InlineResult::text(res))
            },
            InlineCommand::Top => {
                metrics::CMD_TOP_COUNTER.inline.inc();
                dick::top_impl(repos, &config, from_refs, Page::first())
                    .await
                    .map(|top| {
                        let mut res = InlineResult::text(top.lines);
                        res.keyboard = config.features.chats_merging
                            .then_some(build_pagination_keyboard(Page::first(), top.has_more_pages));
                        res
                    })
            },
            InlineCommand::DickOfDay => {
                metrics::CMD_DOD_COUNTER.inline.inc();
                dod::dick_of_day_impl(repos, config, from_refs)
                    .await
                    .map(|res| InlineResult::text(res))
            },
        }
    }
}

pub async fn inline_handler(bot: Bot, query: InlineQuery, repos: Repositories) -> HandlerResult {
    metrics::INLINE_COUNTER.invoked();

    let name = utils::get_full_name(&query.from);
    repos.users.create_or_update(query.from.id, &name).await?;

    let uid = query.from.id.0;
    let lang_code = ensure_lang_code(Some(&query.from));
    let btn_label = t!("inline.results.button", locale = &lang_code);
    let results: Vec<InlineQueryResult> = InlineCommand::iter()
        .map(|cmd| cmd.to_string())
        .map(|key| {
            let title = t!(&format!("inline.results.titles.{key}"), locale = &lang_code);
            let content = InputMessageContent::Text(InputMessageContentText::new(
                t!("inline.results.text", locale = &lang_code)));
            let mut article = InlineQueryResultArticle::new(
                key.clone(), title, content
            );
            let buttons = vec![vec![
                InlineKeyboardButton::callback(&btn_label, format!("{uid}:{key}"))
            ]];
            article.reply_markup.replace(InlineKeyboardMarkup::new(buttons));
            InlineQueryResult::Article(article)
        })
        .collect();

    bot.answer_inline_query(query.id, results)
        .is_personal(true)
        .cache_time(1)
        .await?;
    Ok(())
}

pub async fn inline_chosen_handler(bot: Bot, result: ChosenInlineResult,
                                   repos: Repositories, config: AppConfig) -> HandlerResult {
    metrics::INLINE_COUNTER.finished();

    let maybe_chat_in_sync = result.inline_message_id.as_ref()
        .and_then(try_resolve_chat_id)
        .map(|chat_id| repos.chats.get_chat(chat_id.into()));
    if let Some(chat_in_sync_future) = maybe_chat_in_sync {
        let maybe_chat = chat_in_sync_future
            .map_ok(|res| res.filter(|c| c.chat_id.is_some() && c.chat_instance.is_some()))
            .await?;
        if let Some(chat) = maybe_chat {
            log::debug!("[inline_chosen_handler] chat: {chat:?}, user_id: {}", result.from.id);

            let cmd = InlineCommand::from_str(&result.result_id)?;
            let chat_id = chat.try_into().map_err(|e: NoChatIdError| anyhow!(e))?;
            let from_refs = FromRefs(&result.from, &chat_id);
            let inline_result = cmd.execute(&repos, config, from_refs).await?;

            let inline_message_id = result.inline_message_id
                .ok_or("inline_message_id must be set if the chat_in_sync_future exists")?;
            let mut request = bot.edit_message_text_inline(inline_message_id, inline_result.text);
            request.reply_markup = inline_result.keyboard;
            request.parse_mode.replace(Html);
            request.await?;
        }
    }

    Ok(())
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery,
                              repos: Repositories, config: AppConfig) -> HandlerResult {
    let lang_code = ensure_lang_code(Some(&query.from));
    let mut answer = bot.answer_callback_query(&query.id);

    if let (Some(inline_msg_id), Some(data)) = (query.inline_message_id, query.data) {
        let chat_id = config.features.chats_merging
            .then(|| utils::resolve_inline_message_id(&inline_msg_id))
            .map(|res| match res {
                Ok(info) => ChatIdFull {
                    id: ChatId(info.chat_id),
                    instance: query.chat_instance.clone(),
                }.to_partiality(ChatIdSource::InlineQuery),
                Err(err) => {
                    log::error!("callback_handler couldn't resolve an inline_message_id: {err}");
                    query.chat_instance.clone().into()
                }
            })
            .unwrap_or(query.chat_instance.clone().into());
        log::debug!("[callback_handler] chat_id: {chat_id:?}, user_id: {}", query.from.id);

        let parse_res = parse_callback_data(&data, query.from.id);
        if let Ok(CallbackDataParseResult::Ok(cmd)) = parse_res {
            let from_refs = FromRefs(&query.from, &chat_id);
            let inline_result = cmd.execute(&repos, config, from_refs).await?;
            let mut edit = bot.edit_message_text_inline(inline_msg_id, inline_result.text);
            edit.reply_markup = inline_result.keyboard;
            edit.parse_mode.replace(Html);
            edit.await?;
        } else {
            let key = match parse_res {
                Ok(CallbackDataParseResult::AnotherUser) => "another_user",
                Ok(CallbackDataParseResult::Invalid) => "invalid_data",
                Err(e) => {
                    log::error!("unknown callback data: {e}");
                    "unknown_data"
                }
                Ok(CallbackDataParseResult::Ok(_)) => panic!("unexpected CallbackDataParseResult::Ok(_)")
            };
            let text = t!(&format!("inline.callback.errors.{key}"), locale = &lang_code);
            answer.text.replace(text);
            answer.show_alert.replace(true);
        }
    } else {
        let text = t!("inline.callback.errors.no_data", locale = &lang_code);
        answer.text.replace(text);
        answer.show_alert.replace(true);
    };

    answer.await?;
    Ok(())
}

enum CallbackDataParseResult {
    Ok(InlineCommand),
    AnotherUser,
    Invalid,
}

fn parse_callback_data(data: &str, user_id: UserId) -> Result<CallbackDataParseResult, strum::ParseError> {
    data.split_once(":")
        .map(|(uid, data)| {
            if uid == user_id.0.to_string() {
                InlineCommand::from_str(&data)
                    .map(|cmd| CallbackDataParseResult::Ok(cmd))
            } else {
                Ok(CallbackDataParseResult::AnotherUser)
            }
        })
        .unwrap_or(Ok(CallbackDataParseResult::Invalid))
}

pub(crate) fn try_resolve_chat_id(msg_id: &String) -> Option<ChatId> {
    utils::resolve_inline_message_id(msg_id)
        .or_else(|e| {
            log::error!("couldn't resolve inline_message_id: {e}");
            Err(e)
        })
        .ok()
        .map(|info| ChatId(info.chat_id))
}
