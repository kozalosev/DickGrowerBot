use std::collections::HashSet;
use std::fmt::Debug;
use std::str::FromStr;
use anyhow::{anyhow, Context};
use futures::TryFutureExt;
use once_cell::sync::Lazy;
use rust_i18n::t;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString};
use teloxide::{ApiError, Bot, RequestError};
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::requests::Requester;
use teloxide::types::*;
use teloxide::types::ParseMode::Html;
use crate::config::AppConfig;
use crate::domain::{LanguageCode, Username};
use crate::handlers::{build_pagination_keyboard, dick, dod, FromRefs, HandlerImplResult, HandlerResult, loan, stats, utils, pvp};
use crate::handlers::utils::callbacks::CallbackDataWithPrefix;
use crate::handlers::utils::Incrementor;
use crate::handlers::utils::page::Page;
use crate::metrics;
use crate::repo::{ChatIdFull, NoChatIdError, ChatIdSource, Repositories};

#[derive(Debug, strum_macros::Display, EnumIter, EnumString)]
#[strum(serialize_all = "snake_case")]
enum InlineCommand {
    Grow,
    Top,
    DickOfDay,
    Loan,
    Stats,
}

struct InlineResult {
    text: String,
    keyboard: Option<InlineKeyboardMarkup>,
}

impl <D: CallbackDataWithPrefix> From<HandlerImplResult<D>> for InlineResult {
    fn from(value: HandlerImplResult<D>) -> Self {
        Self {
            text: value.text(),
            keyboard: value.keyboard()
        }
    }
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
    async fn execute(&self, repos: &Repositories, config: AppConfig, incr: Incrementor, from_refs: FromRefs<'_>) -> anyhow::Result<InlineResult> {
        match self {
            InlineCommand::Grow => {
                metrics::CMD_GROW_COUNTER.inline.inc();
                dick::grow_impl(repos, incr, from_refs)
                    .await
                    .map(InlineResult::text)
            },
            InlineCommand::Top => {
                metrics::CMD_TOP_COUNTER.inline.inc();
                dick::top_impl(repos, &config, from_refs, Page::first())
                    .await
                    .map(|top| {
                        let mut res = InlineResult::text(top.lines);
                        res.keyboard = config.features.top_unlimited
                            .then_some(build_pagination_keyboard(Page::first(), top.has_more_pages));
                        res
                    })
            },
            InlineCommand::DickOfDay => {
                metrics::CMD_DOD_COUNTER.inline.inc();
                dod::dick_of_day_impl(config, repos, incr, from_refs)
                    .await
                    .map(InlineResult::text)
            },
            InlineCommand::Loan => {
                metrics::CMD_LOAN_COUNTER.invoked.inline.inc();
                loan::loan_impl(repos, from_refs, config)
                    .await
                    .map(InlineResult::from)
            },
            InlineCommand::Stats => {
                metrics::CMD_STATS.inline.inc();
                stats::chat_stats_impl(repos, from_refs, config.features.pvp)
                    .await
                    .map(InlineResult::text)
            },
        }
    }
}

type ExternalVariantBuilder = fn(&InlineQuery, &LanguageCode, &AppConfig, &Username) -> InlineQueryResult;

struct ExternalVariant {
    result_id: &'static str,
    builder: ExternalVariantBuilder
}

struct ExternalVariants {
    result_ids: HashSet<&'static str>,
    builders: Vec<ExternalVariantBuilder>
}
impl ExternalVariants {
    fn new(variants: &'static [ExternalVariant]) -> Self {
        let result_ids = variants.iter()
            .map(|v| v.result_id)
            .collect();
        let builders = variants.iter()
            .map(|v| v.builder)
            .collect();
        Self { result_ids, builders }
    }
}

static EXTERNAL_VARIANTS: Lazy<ExternalVariants> = Lazy::new(|| ExternalVariants::new(&[
    ExternalVariant {
        result_id: "pvp",
        builder: |query, lang_code, app_config, name| {
            pvp::build_inline_keyboard_article_result(query.from.id, lang_code, name, app_config.pvp_default_bet)
        }
    }
]));

pub async fn inline_handler(bot: Bot, query: InlineQuery, repos: Repositories, app_config: AppConfig) -> HandlerResult {
    metrics::INLINE_COUNTER.invoked();

    let name = utils::get_full_name(&query.from);
    repos.users.create_or_update(query.from.id, &name).await?;

    let uid = query.from.id.0;
    let lang_code = LanguageCode::from_user(&query.from);
    let btn_label = t!("inline.results.button", locale = &lang_code);
    let mut results: Vec<InlineQueryResult> = InlineCommand::iter()
        .map(|cmd| cmd.to_string())
        .filter(|cmd| app_config.command_toggles.enabled(cmd))
        .map(|key| {
            let t_key = format!("inline.results.titles.{key}");
            let title = t!(&t_key, locale = &lang_code);
            let content = InputMessageContent::Text(InputMessageContentText::new(
                t!("inline.results.text", locale = &lang_code)));
            let mut article = InlineQueryResultArticle::new(
                key.clone(), title, content
            );
            let buttons = vec![vec![
                InlineKeyboardButton::callback(btn_label.clone(), format!("{uid}:{key}"))
            ]];
            article.reply_markup.replace(InlineKeyboardMarkup::new(buttons));
            InlineQueryResult::Article(article)
        })
        .collect();
    for builder in &EXTERNAL_VARIANTS.builders {
        results.push(builder(&query, &lang_code, &app_config, &name))
    }

    let mut answer = bot.answer_inline_query(&query.id, results.clone())
        .is_personal(true);
    if cfg!(debug_assertions) {
        answer.cache_time.replace(1);
    }
    answer.await.context(format!("couldn't answer inline query {query:?} with results {results:?}"))?;
    Ok(())
}

pub async fn inline_chosen_handler(bot: Bot, result: ChosenInlineResult,
                                   repos: Repositories, config: AppConfig,
                                   incr: Incrementor) -> HandlerResult {
    metrics::INLINE_COUNTER.finished();

    if EXTERNAL_VARIANTS.result_ids.contains(result.result_id.as_str()) {
        return Ok(())
    }

    let maybe_chat_in_sync = result.inline_message_id.as_ref()
        .and_then(try_resolve_chat_id)
        .map(|chat_id| repos.chats.get_chat(chat_id.into()));
    if let Some(chat_in_sync_future) = maybe_chat_in_sync {
        let maybe_chat = chat_in_sync_future
            .map_ok(|res| res.filter(|c| c.chat_id.is_some() && c.chat_instance.is_some()))
            .await?;
        if let Some(chat) = maybe_chat {
            log::debug!("[inline_chosen_handler] chat: {chat:?}, user_id: {}", result.from.id);

            let cmd = InlineCommand::from_str(&result.result_id)
                .context(format!("couldn't parse inline command '{}'", result.result_id))?;
            let chat_id = chat.try_into().map_err(|e: NoChatIdError| anyhow!(e))?;
            let from_refs = FromRefs(&result.from, &chat_id);
            let inline_result = cmd.execute(&repos, config, incr, from_refs).await?;

            let inline_message_id = result.inline_message_id
                .ok_or("inline_message_id must be set if the chat_in_sync_future exists")?;
            let mut request = bot.edit_message_text_inline(inline_message_id, &inline_result.text);
            request.reply_markup = inline_result.keyboard;
            request.parse_mode.replace(Html);
            request.disable_web_page_preview.replace(true);
            request.await
                .inspect_err(|e| log_text_if_unknown_api_error(&inline_result.text, e))?;
        }
    }

    Ok(())
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery,
                              repos: Repositories, config: AppConfig,
                              incr: Incrementor) -> HandlerResult {
    let lang_code = LanguageCode::from_user(&query.from);
    let mut answer = bot.answer_callback_query(&query.id);

    if let (Some(inline_msg_id), Some(data)) = (&query.inline_message_id, &query.data) {
        let chat_id = config.features.chats_merging
            .then(|| utils::resolve_inline_message_id(inline_msg_id))
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

        let parse_res = parse_callback_data(data, query.from.id);
        if let Ok(CallbackDataParseResult::Ok(cmd)) = parse_res {
            let from_refs = FromRefs(&query.from, &chat_id);
            let inline_result = cmd.execute(&repos, config, incr, from_refs).await?;
            let mut edit = bot.edit_message_text_inline(inline_msg_id, &inline_result.text);
            edit.reply_markup = inline_result.keyboard;
            edit.parse_mode.replace(Html);
            edit.disable_web_page_preview.replace(true);
            edit.await
                .inspect_err(|e| log_text_if_unknown_api_error(&inline_result.text, e))?;
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
            let t_key = format!("inline.callback.errors.{key}");
            let text = t!(&t_key, locale = &lang_code).to_string();
            answer.text.replace(text);
            answer.show_alert.replace(true);
        }
    } else {
        let text = t!("inline.callback.errors.no_data", locale = &lang_code).to_string();
        answer.text.replace(text);
        answer.show_alert.replace(true);
    };

    answer.await.context(format!("couldn't answer a callback query {query:?}"))?;
    Ok(())
}

enum CallbackDataParseResult {
    Ok(InlineCommand),
    AnotherUser,
    Invalid,
}

fn parse_callback_data(data: &str, user_id: UserId) -> Result<CallbackDataParseResult, strum::ParseError> {
    data.split_once(':')
        .map(|(uid, data)| {
            if uid == user_id.0.to_string() {
                InlineCommand::from_str(data)
                    .map(CallbackDataParseResult::Ok)
                    .inspect_err(|err| log::error!("couldn't parse callback data '{data}': {err}"))
            } else {
                Ok(CallbackDataParseResult::AnotherUser)
            }
        })
        .unwrap_or(Ok(CallbackDataParseResult::Invalid))
}

#[allow(clippy::ptr_arg)]
pub(crate) fn try_resolve_chat_id(msg_id: &String) -> Option<ChatId> {
    utils::resolve_inline_message_id(msg_id)
        .inspect_err(|e| log::error!("couldn't resolve inline_message_id: {e}"))
        .ok()
        .map(|info| ChatId(info.chat_id))
}

// TODO: move to mod.rs and use in message handlers too
fn log_text_if_unknown_api_error(text: &str, err: &RequestError) {
    if let RequestError::Api(ApiError::Unknown(_)) = err {
        log::error!("Couldn't send an answer: {text}")
    }
}
