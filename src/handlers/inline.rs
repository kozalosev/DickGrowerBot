use std::str::FromStr;
use anyhow::anyhow;
use rust_i18n::t;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString};
use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::*;
use teloxide::types::ParseMode::Html;
use crate::config::AppConfig;
use crate::handlers::{build_pagination_keyboard, dick, dod, ensure_lang_code, FromRefs, HandlerResult, utils};
use crate::handlers::utils::page::Page;
use crate::metrics;
use crate::repo::Repositories;

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
                dick::top_impl(repos, config, from_refs, Page::first())
                    .await
                    .and_then(|top| top.ok_or(anyhow!("top must not be None via inline mode")))
                    .map(|top| {
                        let mut res = InlineResult::text(top.lines);
                        res.keyboard = Some(build_pagination_keyboard(Page::first(), top.has_more_pages));
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

    let mut answer = bot.answer_inline_query(query.id, results);
    answer.cache_time = Some(1);
    answer.await?;
    Ok(())
}

pub async fn inline_chosen_handler() -> HandlerResult {
    metrics::INLINE_COUNTER.finished();
    Ok(())
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery,
                              repos: Repositories, config: AppConfig) -> HandlerResult {
    let lang_code = ensure_lang_code(Some(&query.from));
    let chat_id = query.chat_instance.into();
    let from_refs = FromRefs(&query.from, &chat_id);
    let mut answer = bot.answer_callback_query(&query.id);

    if let (Some(inline_msg_id), Some(data)) = (query.inline_message_id, query.data) {
        let parse_res = parse_callback_data(&data, query.from.id);
        if let Ok(CallbackDataParseResult::Ok(cmd)) = parse_res {
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
