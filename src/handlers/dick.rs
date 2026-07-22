use std::future::IntoFuture;

use anyhow::{anyhow, Context};
use chrono::{Datelike, Utc};
use futures::future::join;
use futures::TryFutureExt;
use num_traits::ToPrimitive;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, ParseMode, ReplyMarkup};
use teloxide::types::{User as TeloxideUser};
use crate::config::{AppConfig, MessageGroup};
use crate::{metrics, reply_html, repo};
use crate::domain::objects::GrowthResult;
use crate::domain::primitives::chat::ChatIdPartiality;
use crate::domain::primitives::{LanguageCode, Username, Offset, Page, UserId, DaysCount, InvalidPage};
use crate::handlers::{HandlerResult, TaggedReply, reply_html, utils};
use crate::handlers::utils::{callbacks, Incrementor, SelfDestructionService};

const TOMORROW_SQL_CODE: &str = "GD0E1";
const CALLBACK_PREFIX_TOP_PAGE: &str = "top:page:";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum DickCommands {
    #[command(description = "grow")]
    Grow,
    #[command(description = "top")]
    Top,
}

pub async fn dick_cmd_handler(
    bot: Bot,
    msg: Message,
    cmd: DickCommands,
    repos: repo::Repositories,
    incr: Incrementor,
    config: AppConfig,
    lang_code: LanguageCode,
    self_destruction: SelfDestructionService,
) -> HandlerResult {
    let from = msg.from.as_ref().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);
    match cmd {
        DickCommands::Grow => {
            // A real growth is a permanent event; only the "come back tomorrow" status is
            // scheduled (as a Notice). `grow_impl` tells the two apart via the reply group.
            metrics::CMD_GROW_COUNTER.chat.inc();
            let reply = grow_impl(&repos, incr, from_refs, lang_code.clone()).await?;
            let sent = reply_html!(bot.clone(), msg, reply.text);
            self_destruction.schedule(&bot, &sent, reply.group, &lang_code);
        },
        DickCommands::Top => {
            metrics::CMD_TOP_COUNTER.chat.inc();
            let top = top_impl(&repos, &config, from_refs, lang_code.clone(), Page::first()).await?;
            let mut request = reply_html(bot.clone(), &msg, top.lines);
            if top.has_more_pages && config.features.top_unlimited {
                let keyboard = ReplyMarkup::InlineKeyboard(build_pagination_keyboard(Page::first(), top.has_more_pages));
                request.reply_markup.replace(keyboard);
            }
            let sent = request.await.context(format!("failed for {msg:?}"))?;
            self_destruction.schedule(&bot, &sent, MessageGroup::Report, &lang_code);
        }
    };
    Ok(())
}

pub struct FromRefs<'a>(pub &'a TeloxideUser, pub &'a ChatIdPartiality);

pub(crate) async fn grow_impl(
    repos: &repo::Repositories,
    incr: Incrementor,
    from_refs: FromRefs<'_>,
    lang_code: LanguageCode,
) -> anyhow::Result<TaggedReply> {
    let (from, chat_id) = (from_refs.0, from_refs.1);
    let uid = UserId::from(from);
    let name = utils::get_full_name(from);
    let user = repos.users.create_or_update(uid, &name).await?;
    let days_since_registration = Utc::now() - user.created_at;
    let days_since_registration = days_since_registration.num_days().to_u32()
        .map(DaysCount::new)
        .ok_or_else(|| anyhow!("days since registration are too much: {days_since_registration}"))?;
    let increment = incr.growth_increment(uid, chat_id.kind(), days_since_registration).await;
    let grow_result = repos.dicks.create_or_grow(uid, chat_id, increment.total).await;

    let (main_part, group) = match grow_result {
        Ok(GrowthResult { new_length, pos_in_top }) => {
            let event_key = if increment.total.value().is_negative() { "shrunk" } else { "grown" };
            let event_template = format!("commands.grow.direction.{event_key}");
            let event = t!(&event_template, locale = &lang_code);
            let answer = t!("commands.grow.result", locale = &lang_code,
                event = event, incr = increment.total.value().abs(), length = new_length);
            let perks_part = increment.perks_part_of_answer(&lang_code);
            let text = if let Some(pos) = pos_in_top {
                let position = t!("commands.grow.position", locale = &lang_code, pos = pos);
                format!("{answer}\n{position}{perks_part}")
            } else {
                format!("{answer}{perks_part}")
            };
            (text, MessageGroup::Event)
        },
        Err(e) => {
            let db_err = e.downcast::<sqlx::Error>()?;
            if let sqlx::Error::Database(e) = db_err {
                let text = e.code()
                    .filter(|c| c == TOMORROW_SQL_CODE)
                    .map(|_| t!("commands.grow.tomorrow", locale = &lang_code).to_string())
                    .ok_or(anyhow!(e))?;
                (text, MessageGroup::Notice)
            } else {
                Err(db_err)?
            }
        }
    };
    let time_left_part = utils::date::get_time_till_next_day_string(&lang_code);
    Ok(TaggedReply { text: format!("{main_part}{time_left_part}"), group })
}

pub(crate) struct Top {
    pub lines: String,
    pub(crate) has_more_pages: bool,
}

impl Top {
    fn from(s: impl ToString) -> Self {
        Self {
            lines: s.to_string(),
            has_more_pages: false,
        }
    }

    fn with_more_pages(s: impl ToString) -> Self {
        Self {
            lines: s.to_string(),
            has_more_pages: true,
        }
    }
}

pub(crate) async fn top_impl(
    repos: &repo::Repositories,
    config: &AppConfig,
    from_refs: FromRefs<'_>,
    lang_code: LanguageCode,
    page: Page,
) -> anyhow::Result<Top> {
    let (from, chat_id) = (from_refs.0, from_refs.1.kind());
    let offset = Offset::calculate(page, config.top_limit);
    let query_limit = (config.top_limit + 1)?; // fetch +1 row to know whether more rows exist or not
    let dicks = repos.dicks.get_top(&chat_id, offset, query_limit).await?;
    let has_more_pages = (dicks.len() as i16) > config.top_limit.value();
    let mut any_inactive = false;
    let lines = dicks.into_iter()
        .take(config.top_limit.value() as usize)
        .enumerate()
        .map(|(i, d)| {
            let escaped_name = Username::new(d.owner_name).escaped();
            let name = if d.owner_uid == from.id {
                format!("<u>{escaped_name}</u>")
            } else {
                escaped_name
            };
            let now = Utc::now();
            let inactive = (now - d.grown_at).num_days() > config.inactivity_days.value() as i64;
            let can_grow = now.num_days_from_ce() > d.grown_at.num_days_from_ce();
            let pos = d.position.map(|p| p.value() as i64).unwrap_or((i+1) as i64);
            let mut line = t!("commands.top.line", locale = &lang_code,
                n = pos, name = name, length = d.length).to_string();
            if inactive {
                any_inactive = true;
                line.push_str(" [~]")
            } else if can_grow {
                line.push_str(" [+]")
            };
            line
        })
        .collect::<Vec<String>>();

    let res = if lines.is_empty() {
        Top::from(t!("commands.top.empty", locale = &lang_code))
    } else {
        let title = t!("commands.top.title", locale = &lang_code);
        let ending = t!("commands.top.ending", locale = &lang_code);
        let inactive_hint = if any_inactive {
            format!("\n{}", t!("commands.top.ending_inactive", locale = &lang_code, days = config.inactivity_days))
        } else {
            String::new()
        };
        let text = format!("{}\n\n{}\n\n{}{}", title, lines.join("\n"), ending, inactive_hint);
        if has_more_pages {
            Top::with_more_pages(text)
        } else {
            Top::from(text)
        }
    };
    Ok(res)
}

pub fn page_callback_filter(query: CallbackQuery) -> bool {
    query.data
        .filter(|d| d.starts_with(CALLBACK_PREFIX_TOP_PAGE))
        .is_some()
}

pub async fn page_callback_handler(
    bot: Bot,
    q: CallbackQuery,
    config: AppConfig,
    repos: repo::Repositories,
    lang_code: LanguageCode,
) -> HandlerResult {
    let edit_msg_req_params = callbacks::get_params_for_message_edit(&q)?;
    if !config.features.top_unlimited {
        return answer_callback_feature_disabled(bot, &q, edit_msg_req_params, lang_code).await
    }

    let page = q.data.as_ref()
        .ok_or(InvalidPage::message("no data"))
        .and_then(|d| d.strip_prefix(CALLBACK_PREFIX_TOP_PAGE)
            .map(str::to_owned)
            .ok_or(InvalidPage::for_value(d, "invalid prefix")))
        .and_then(|r| r.parse::<i16>()
            .map_err(|e| InvalidPage::for_value(&r, e)))
        .and_then(|value| Page::new(value)
            .map_err(|e| InvalidPage::for_value(&value.to_string(), e)))
        .map_err(|e| anyhow!(e))?;
    let chat_id_kind = edit_msg_req_params.clone().into();
    let chat_id_partiality = ChatIdPartiality::Specific(chat_id_kind);
    let from_refs = FromRefs(&q.from, &chat_id_partiality);
    let top = top_impl(&repos, &config, from_refs, lang_code, page).await?;

    let keyboard = build_pagination_keyboard(page, top.has_more_pages);
    let (answer_callback_query_result, edit_message_result) = match &edit_msg_req_params {
        callbacks::EditMessageReqParamsKind::Chat(chat_id, message_id) => {
            let mut edit_message_text_req = bot.edit_message_text(*chat_id, *message_id, top.lines);
            edit_message_text_req.parse_mode.replace(ParseMode::Html);
            edit_message_text_req.reply_markup.replace(keyboard);
            join(
                bot.answer_callback_query(q.id.clone()).into_future(),
                edit_message_text_req.into_future().map_ok(|_| ())
            ).await
        },
        callbacks::EditMessageReqParamsKind::Inline { inline_message_id, .. } => {
            let mut edit_message_text_inline_req = bot.edit_message_text_inline(inline_message_id, top.lines);
            edit_message_text_inline_req.parse_mode.replace(ParseMode::Html);
            edit_message_text_inline_req.reply_markup.replace(keyboard);
            join(
                bot.answer_callback_query(q.id.clone()).into_future(),
                edit_message_text_inline_req.into_future().map_ok(|_| ())
            ).await
        }
    };
    answer_callback_query_result.context(format!("failed to answer a callback query {q:?}"))?;
    edit_message_result.context(format!("failed to edit the message of {edit_msg_req_params:?}"))?;
    Ok(())
}

pub fn build_pagination_keyboard(page: Page, has_more_pages: bool) -> InlineKeyboardMarkup {
    let mut buttons = Vec::new();
    if page > 0 {
        let prev_page = (page - 1).expect("the page is positive here, so the previous one is valid");
        buttons.push(InlineKeyboardButton::callback("⬅️", format!("{CALLBACK_PREFIX_TOP_PAGE}{prev_page}")))
    }
    if has_more_pages {
        let next_page = (page + 1).expect("the page increment saturates, so it always stays valid");
        buttons.push(InlineKeyboardButton::callback("➡️", format!("{CALLBACK_PREFIX_TOP_PAGE}{next_page}")))
    }
    InlineKeyboardMarkup::new(vec![buttons])
}

async fn answer_callback_feature_disabled(
    bot: Bot,
    q: &CallbackQuery,
    edit_msg_req_params: callbacks::EditMessageReqParamsKind,
    lang_code: LanguageCode,
) -> HandlerResult {
    let mut answer = bot.answer_callback_query(q.id.clone());
    answer.show_alert.replace(true);
    answer.text.replace(t!("errors.feature_disabled", locale = &lang_code).to_string());
    answer.await?;

    match edit_msg_req_params {
        callbacks::EditMessageReqParamsKind::Chat(chat_id, message_id) =>
            bot.edit_message_reply_markup(chat_id, message_id)
                .await.map(|_| ())?,
        callbacks::EditMessageReqParamsKind::Inline { inline_message_id, .. } =>
            bot.edit_message_reply_markup_inline(inline_message_id)
                .await.map(|_| ())?
    };
    Ok(())
}
