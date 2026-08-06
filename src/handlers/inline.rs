use autometrics::autometrics;
use std::collections::HashSet;
use std::fmt::Debug;
use std::str::FromStr;
use anyhow::{anyhow, Context};
use futures::TryFutureExt;
use once_cell::sync::Lazy;
use rust_i18n::t;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString};
use teloxide::Bot;
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::requests::Requester;
use teloxide::types::*;
use teloxide::types::ParseMode::Html;
use crate::config::{AppConfig, MessageGroup};
use crate::domain::objects::InlineMessageIdInfo;
use crate::domain::primitives::{LanguageCode, Page, UserId as DomainUserId, Username};
use crate::domain::primitives::chat::{ChatIdFull, ChatIdSource, TelegramChatInstanceId};
use crate::handlers::{banned_until_of, dick, dod, FromRefs, HandlerDeps, HandlerImplResult, HandlerResult, loan, shrink, stats, utils, pvp};
use crate::handlers::utils::callbacks::CallbackDataWithPrefix;
use crate::handlers::utils::Incrementor;
use crate::metrics;
use crate::repo::{NoChatIdError, Repositories};

#[derive(Debug, strum_macros::Display, EnumIter, EnumString)]
#[strum(serialize_all = "snake_case")]
enum InlineCommand {
    Grow,
    Top,
    DickOfDay,
    Loan,
    Stats,
    Shrinks,
}

struct InlineResult {
    text: String,
    keyboard: Option<InlineKeyboardMarkup>,
    /// The lifetime of the message, as with [`TaggedReply`](crate::handlers::TaggedReply) — it
    /// decides whether the message is later replaced with the self-destruction placeholder.
    group: MessageGroup,
}

impl InlineResult {
    fn text(value: String, group: MessageGroup) -> Self {
        Self {
            text: value,
            keyboard: None,
            group,
        }
    }

    fn with_keyboard<D: CallbackDataWithPrefix>(value: HandlerImplResult<D>, group: MessageGroup) -> Self {
        Self {
            text: value.text(),
            keyboard: value.keyboard(),
            group,
        }
    }
}

impl InlineCommand {
    async fn execute(
        &self,
        repos: &Repositories,
        config: AppConfig,
        incr: Incrementor,
        from_refs: FromRefs<'_>,
        lang_code: &LanguageCode,
    ) -> anyhow::Result<InlineResult> {
        match self {
            InlineCommand::Grow => {
                metrics::CMD_GROW_COUNTER.inline.inc();
                dick::grow_impl(repos, incr, from_refs, lang_code)
                    .await
                    .map(|reply| InlineResult::text(reply.text, reply.group))
            },
            InlineCommand::Top => {
                metrics::CMD_TOP_COUNTER.inline.inc();
                dick::top_impl(repos, &config, from_refs, lang_code, Page::first())
                    .await
                    .map(|top| {
                        let mut res = InlineResult::text(top.lines, MessageGroup::Report);
                        res.keyboard = config.features.top_unlimited
                            .then_some(dick::build_pagination_keyboard(Page::first(), top.has_more_pages));
                        res
                    })
            },
            InlineCommand::DickOfDay => {
                metrics::CMD_DOD_COUNTER.inline.inc();
                dod::dick_of_day_impl(config, repos, incr, from_refs, lang_code)
                    .await
                    .map(|reply| InlineResult::text(reply.text, reply.group))
            },
            InlineCommand::Loan => {
                metrics::CMD_LOAN_COUNTER.invoked.inline.inc();
                loan::loan_impl(repos, from_refs, config, lang_code)
                    .await
                    .map(|result| InlineResult::with_keyboard(result, MessageGroup::Application))
            },
            InlineCommand::Stats => {
                metrics::CMD_STATS.inline.inc();
                stats::chat_stats_impl(repos, from_refs, config.features.pvp, lang_code)
                    .await
                    .map(|text| InlineResult::text(text, MessageGroup::Report))
            },
            InlineCommand::Shrinks => {
                metrics::CMD_SHRINKS.inc();
                shrink::recent_shrinks_impl(repos, from_refs, &config, lang_code)
                    .await
                    .map(|(page, keyboard)| {
                        let mut res = InlineResult::text(page.lines, MessageGroup::Report);
                        res.keyboard = keyboard;
                        res
                    })
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
            pvp::build_inline_keyboard_article_result(DomainUserId::from(&query.from), lang_code, name, app_config.pvp_default_bet)
        }
    }
]));

#[autometrics]
#[tracing::instrument(skip_all, fields(uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn inline_handler(
    bot: Bot,
    query: InlineQuery,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, config: app_config, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::INLINE_COUNTER.invoked();

    let name = utils::get_full_name(&query.from);
    match repos.users.create_or_update(DomainUserId::from(&query.from), &name).await {
        Ok(_) => {},
        Err(e) if banned_until_of(&e).is_some() => {
            bot.answer_inline_query(query.id, vec![])
                .is_personal(true)
                .cache_time(1)
                .await?;
            return Ok(())
        },
        Err(e) => return Err(e.into()),
    }

    let uid = query.from.id.0;
    let btn_label = t!("inline.results.button", locale = &lang_code);
    // Everywhere else the tap is a one-off — it teaches the bot the chat, and the next invocation
    // resolves on its own. A legacy group never becomes identifiable from an inline query, so
    // there the tap is permanent and the text has to say so instead of promising it'll get better.
    let text_key = match query.chat_type {
        Some(ChatType::Group) => "inline.results.text_legacy_group",
        _ => "inline.results.text",
    };
    let mut results: Vec<InlineQueryResult> = InlineCommand::iter()
        // The `shrinks` command only makes sense when the daily shrink feature is on.
        .filter(|cmd| !matches!(cmd, InlineCommand::Shrinks) || app_config.daily_shrink.enabled())
        .map(|cmd| cmd.to_string())
        .filter(|cmd| app_config.command_toggles.enabled(cmd))
        .map(|key| {
            let t_key = format!("inline.results.titles.{key}");
            let title = t!(&t_key, locale = &lang_code);
            let content = InputMessageContent::Text(InputMessageContentText::new(
                t!(text_key, locale = &lang_code)));
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

    let mut answer = bot.answer_inline_query(query.id.clone(), results.clone())
        .is_personal(true);
    if cfg!(debug_assertions) {
        answer.cache_time.replace(1);
    }
    answer.await.context(format!("couldn't answer inline query {query:?} with results {results:?}"))?;
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(uid = result.from.id.0, lang_code = tracing::field::Empty))]
pub async fn inline_chosen_handler(
    bot: Bot,
    result: ChosenInlineResult,
    incr: Incrementor,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, config, self_destruction, lang_resolver } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::INLINE_COUNTER.finished();

    if EXTERNAL_VARIANTS.result_ids.contains(result.result_id.as_str()) {
        return Ok(())
    }

    let maybe_chat_in_sync = result.inline_message_id.as_deref()
        .and_then(utils::try_resolve_chat_id)
        .map(|chat_id| repos.chats.get_chat(chat_id.into()));
    if let Some(chat_in_sync_future) = maybe_chat_in_sync {
        let maybe_chat = chat_in_sync_future
            .map_ok(|res| res.filter(|c| c.chat_id.is_some() && c.chat_instance.is_some()))
            .await?;
        if let Some(chat) = maybe_chat {
            tracing::debug!(chat = ?chat, uid = result.from.id.0, "handling a chosen inline result");

            let cmd = InlineCommand::from_str(&result.result_id)
                .context(format!("couldn't parse inline command '{}'", result.result_id))?;
            let chat_id = chat.try_into().map_err(|e: NoChatIdError| anyhow!(e))?;
            let from_refs = FromRefs(&result.from, &chat_id);
            let inline_result = cmd.execute(&repos, config, incr, from_refs, &lang_code).await?;

            let inline_message_id = result.inline_message_id
                .ok_or("inline_message_id must be set if the chat_in_sync_future exists")?;
            let mut request = bot.edit_message_text_inline(inline_message_id.as_str(), inline_result.text);
            request.reply_markup = inline_result.keyboard;
            request.parse_mode.replace(Html);
            request.disable_web_page_preview.replace(true);
            request.await?;

            self_destruction.schedule_inline(&inline_message_id, inline_result.group, &lang_code).await;
        }
    }

    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn inline_callback_handler(
    bot: Bot,
    query: CallbackQuery,
    incr: Incrementor,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, config, self_destruction, lang_resolver } = deps;
    let lang_code = lang_resolver.execute().await;
    let mut answer = bot.answer_callback_query(query.id.clone());

    if let (Some(inline_msg_id), Some(data)) = (&query.inline_message_id, &query.data) {
        let chat_id = config.features.chats_merging
            .then(|| utils::resolve_inline_message_id(inline_msg_id))
            .map(|res| match res {
                Ok(InlineMessageIdInfo { chat_id: Some(id), .. }) => ChatIdFull {
                    id,
                    instance: TelegramChatInstanceId::new(query.chat_instance.clone()),
                }.to_partiality(ChatIdSource::InlineQuery),
                Ok(InlineMessageIdInfo { chat_id: None, .. }) => query.chat_instance.clone().into(),
                Err(err) => {
                    tracing::error!(error = %err, "couldn't resolve the inline_message_id");
                    query.chat_instance.clone().into()
                }
            })
            .unwrap_or(query.chat_instance.clone().into());
        tracing::debug!(chat_id = ?chat_id, uid = query.from.id.0, "handling a callback query");

        let parse_res = parse_callback_data(data, query.from.id);
        if let Ok(CallbackDataParseResult::Ok(cmd)) = parse_res {
            let from_refs = FromRefs(&query.from, &chat_id);
            let inline_result = cmd.execute(&repos, config, incr, from_refs, &lang_code).await?;
            let mut edit = bot.edit_message_text_inline(inline_msg_id, inline_result.text);
            edit.reply_markup = inline_result.keyboard;
            edit.parse_mode.replace(Html);
            edit.disable_web_page_preview.replace(true);
            edit.await?;

            // Legacy groups send no chosen result. For them this is the first place the bot sees
            // the id. If the message is already scheduled, the insert does nothing, so paging does
            // not move the time it will go at.
            self_destruction.schedule_inline(inline_msg_id, inline_result.group, &lang_code).await;
        } else {
            let key = match parse_res {
                Ok(CallbackDataParseResult::AnotherUser) => "another_user",
                Ok(CallbackDataParseResult::Invalid) => "invalid_data",
                Err(e) => {
                    tracing::error!(error = %e, "unknown callback data");
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
                    .inspect_err(|err| tracing::error!(data = %data, error = %err, "couldn't parse the callback data"))
            } else {
                Ok(CallbackDataParseResult::AnotherUser)
            }
        })
        .unwrap_or(Ok(CallbackDataParseResult::Invalid))
}
