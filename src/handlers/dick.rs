use std::future::IntoFuture;
use anyhow::anyhow;
use chrono::{Datelike, Utc};
use futures::future::join;
use futures::TryFutureExt;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId, ParseMode, ReplyMarkup, User};
use page::{InvalidPage, Page};
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html, utils};
use crate::{config, metrics, repo};
use crate::handlers::utils::{Incrementor, page};
use crate::repo::{ChatIdKind, ChatIdPartiality};

const TOMORROW_SQL_CODE: &str = "GD0E1";
const LTR_MARK: char = '\u{200E}';
const CALLBACK_PREFIX_TOP_PAGE: &str = "top:page:";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum DickCommands {
    #[command(description = "grow")]
    Grow,
    #[command(description = "top")]
    Top,
}

pub async fn dick_cmd_handler(bot: Bot, msg: Message, cmd: DickCommands,
                              repos: repo::Repositories, incr: Incrementor,
                              config: config::AppConfig) -> HandlerResult {
    let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);
    match cmd {
        DickCommands::Grow => {
            metrics::CMD_GROW_COUNTER.chat.inc();
            let answer = grow_impl(&repos, incr, from_refs).await?;
            reply_html(bot, msg, answer)
        },
        DickCommands::Top => {
            metrics::CMD_TOP_COUNTER.chat.inc();
            let top = top_impl(&repos, &config, from_refs, Page::first()).await?;
            let mut request = reply_html(bot, msg, top.lines);
            if top.has_more_pages && config.features.top_unlimited {
                let keyboard = ReplyMarkup::InlineKeyboard(build_pagination_keyboard(Page::first(), top.has_more_pages));
                request.reply_markup.replace(keyboard);
            }
            request
        }
    }.await?;
    Ok(())
}

pub struct FromRefs<'a>(pub &'a User, pub &'a ChatIdPartiality);

pub(crate) async fn grow_impl(repos: &repo::Repositories, incr: Incrementor, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
    let (from, chat_id) = (from_refs.0, from_refs.1);
    let name = utils::get_full_name(from);
    let user = repos.users.create_or_update(from.id, &name).await?;
    let days_since_registration = (Utc::now() - user.created_at).num_days() as u32;
    let increment = incr.growth_increment(from.id, chat_id.kind(), days_since_registration).await;
    let grow_result = repos.dicks.create_or_grow(from.id, chat_id, increment.total).await;
    let lang_code = ensure_lang_code(Some(from));

    let main_part = match grow_result {
        Ok(repo::GrowthResult { new_length, pos_in_top }) => {
            let event_key = if increment.total.is_negative() { "shrunk" } else { "grown" };
            let event = t!(&format!("commands.grow.direction.{event_key}"), locale = &lang_code);
            let answer = t!("commands.grow.result", locale = &lang_code,
                event = event, incr = increment.total.abs(), length = new_length);
            if let Some(pos) = pos_in_top {
                let position = t!("commands.grow.position", locale = &lang_code, pos = pos);
                format!("{answer}\n{position}")
            } else {
                answer
            }
        },
        Err(e) => {
            let db_err = e.downcast::<sqlx::Error>()?;
            if let sqlx::Error::Database(e) = db_err {
                e.code()
                    .filter(|c| c == TOMORROW_SQL_CODE)
                    .map(|_| t!("commands.grow.tomorrow", locale = &lang_code))
                    .ok_or(anyhow!(e))?
            } else {
                Err(db_err)?
            }
        }
    };
    let perks_part = increment.perks_part_of_answer(&lang_code);
    let time_left_part = utils::date::get_time_till_next_day_string(&lang_code);
    Ok(format!("{main_part}{perks_part}{time_left_part}"))
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

pub(crate) async fn top_impl(repos: &repo::Repositories, config: &config::AppConfig, from_refs: FromRefs<'_>,
                             page: Page) -> anyhow::Result<Top> {
    let (from, chat_id) = (from_refs.0, from_refs.1.kind());
    let lang_code = ensure_lang_code(Some(from));
    let offset = page * config.top_limit;
    let query_limit = config.top_limit + 1; // fetch +1 row to know whether more rows exist or not
    let dicks = repos.dicks.get_top(&chat_id, offset, query_limit).await?;
    let has_more_pages = dicks.len() as u32 > config.top_limit;
    let lines = dicks.into_iter()
        .take(config.top_limit as usize)
        .enumerate()
        .map(|(i, d)| {
            let ltr_name = format!("{LTR_MARK}{}{LTR_MARK}", d.owner_name);
            let name = teloxide::utils::html::escape(&ltr_name);
            let can_grow = chrono::Utc::now().num_days_from_ce() > d.grown_at.num_days_from_ce();
            let pos = d.position.unwrap_or((i+1) as i64);
            let line = t!("commands.top.line", locale = &lang_code,
                n = pos, name = name, length = d.length);
            if can_grow {
                format!("{line} [+]")
            } else {
                line
            }
        })
        .collect::<Vec<String>>();

    let res = if lines.is_empty() {
        Top::from(t!("commands.top.empty", locale = &lang_code))
    } else {
        let title = t!("commands.top.title", locale = &lang_code);
        let ending = t!("commands.top.ending", locale = &lang_code);
        let text = format!("{}\n\n{}\n\n{}", title, lines.join("\n"), ending);
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

#[derive(Clone)]
enum EditMessageReqParamsKind {
    Chat(ChatId, MessageId),
    Inline { chat_instance: String, inline_message_id: String },
}

#[allow(clippy::from_over_into)]
impl Into<ChatIdKind> for EditMessageReqParamsKind {
    fn into(self) -> ChatIdKind {
        match self {
            EditMessageReqParamsKind::Chat(chat_id, _) => chat_id.into(),
            EditMessageReqParamsKind::Inline { chat_instance, .. } => chat_instance.into(),
        }
    }
}

pub async fn page_callback_handler(bot: Bot, q: CallbackQuery,
                              config: config::AppConfig, repos: repo::Repositories) -> HandlerResult {
    let edit_msg_req_params = q.message.as_ref()
        .map(|m| EditMessageReqParamsKind::Chat(m.chat.id, m.id))
        .or(q.inline_message_id.as_ref().map(|inline_message_id| EditMessageReqParamsKind::Inline {
            chat_instance: q.chat_instance.clone(),
            inline_message_id: inline_message_id.clone()
        }))
        .ok_or("no message")?;

    if !config.features.top_unlimited {
        return answer_callback_feature_disabled(bot, q, edit_msg_req_params).await
    }

    let page = q.data
        .ok_or(InvalidPage::message("no data"))
        .and_then(|d| d.strip_prefix(CALLBACK_PREFIX_TOP_PAGE)
            .map(str::to_owned)
            .ok_or(InvalidPage::for_value(&d, "invalid prefix")))
        .and_then(|r| r.parse()
            .map_err(|e| InvalidPage::for_value(&r, e)))
        .map(Page)
        .map_err(|e| anyhow!(e))?;
    let chat_id_kind = edit_msg_req_params.clone().into();
    let chat_id_partiality = ChatIdPartiality::Specific(chat_id_kind);
    let from_refs = FromRefs(&q.from, &chat_id_partiality);
    let top = top_impl(&repos, &config, from_refs, page).await?;

    let keyboard = build_pagination_keyboard(page, top.has_more_pages);
    let (answer_callback_query_result, edit_message_result) = match edit_msg_req_params {
        EditMessageReqParamsKind::Chat(chat_id, message_id) => {
            let mut edit_message_text_req = bot.edit_message_text(chat_id, message_id, top.lines);
            edit_message_text_req.parse_mode.replace(ParseMode::Html);
            edit_message_text_req.reply_markup.replace(keyboard);
            join(
                bot.answer_callback_query(q.id).into_future(),
                edit_message_text_req.into_future().map_ok(|_| ())
            ).await
        },
        EditMessageReqParamsKind::Inline { inline_message_id, .. } => {
            let mut edit_message_text_inline_req = bot.edit_message_text_inline(inline_message_id, top.lines);
            edit_message_text_inline_req.parse_mode.replace(ParseMode::Html);
            edit_message_text_inline_req.reply_markup.replace(keyboard);
            join(
                bot.answer_callback_query(q.id).into_future(),
                edit_message_text_inline_req.into_future().map_ok(|_| ())
            ).await
        }
    };
    answer_callback_query_result?;
    edit_message_result?;
    Ok(())
}

pub fn build_pagination_keyboard(page: Page, has_more_pages: bool) -> InlineKeyboardMarkup {
    let mut buttons = Vec::new();
    if page > 0 {
        buttons.push(InlineKeyboardButton::callback("⬅️", format!("{CALLBACK_PREFIX_TOP_PAGE}{}", page - 1)))
    }
    if has_more_pages {
        buttons.push(InlineKeyboardButton::callback("➡️", format!("{CALLBACK_PREFIX_TOP_PAGE}{}", page + 1)))
    }
    InlineKeyboardMarkup::new(vec![buttons])
}

async fn answer_callback_feature_disabled(bot: Bot, q: CallbackQuery, edit_msg_req_params: EditMessageReqParamsKind) -> HandlerResult {
    let lang_code = ensure_lang_code(Some(&q.from));

    let mut answer = bot.answer_callback_query(q.id);
    answer.show_alert.replace(true);
    answer.text.replace(t!("errors.feature_disabled", locale = &lang_code));
    answer.await?;

    match edit_msg_req_params {
        EditMessageReqParamsKind::Chat(chat_id, message_id) =>
            bot.edit_message_reply_markup(chat_id, message_id)
                .await.map(|_| ())?,
        EditMessageReqParamsKind::Inline { inline_message_id, .. } =>
            bot.edit_message_reply_markup_inline(inline_message_id)
                .await.map(|_| ())?
    };
    Ok(())
}
