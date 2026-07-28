use std::future::IntoFuture;

use anyhow::{anyhow, Context};
use derive_more::Display;
use futures::future::join;
use futures::TryFutureExt;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, ParseMode};
use crate::config::AppConfig;
use crate::domain::primitives::{DaysCount, LanguageCode, Offset, Page};
use crate::domain::primitives::chat::ChatIdKind;
use crate::handlers::{answer_callback_feature_disabled, build_pagination_keyboard, FromRefs, HandlerResult};
use crate::handlers::utils::callbacks;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::repo::Repositories;

/// One rendered page of a shrink list — either the proactive broadcast or the inline `shrinks`
/// command. Both read from the same [`crate::repo::Shrinks::get_recent_shrinks`] query and only
/// differ in their day window and locale template, see [`ShrinkView`].
pub(crate) struct ShrinksPage {
    pub lines: String,
    pub(crate) has_more_pages: bool,
}

/// Distinguishes the two shrink-list views sharing this module's paging: the day window and the
/// locale template both hinge on which one a page belongs to.
#[derive(Display, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShrinkView {
    /// The proactive midnight broadcast — today's shrinks only.
    #[display("b")]
    Broadcast,
    /// The inline `shrinks` command — everything within `history_window_days`.
    #[display("r")]
    Recent,
}

impl ShrinkView {
    fn days(self, config: &AppConfig) -> DaysCount {
        match self {
            ShrinkView::Broadcast => DaysCount::new(1),
            ShrinkView::Recent => config.stale_dicks_shrinking.history_window_days,
        }
    }

    fn template_prefix(self) -> &'static str {
        match self {
            ShrinkView::Broadcast => "commands.shrink.notification",
            ShrinkView::Recent => "commands.shrink.recent",
        }
    }

    /// The raw callback-data prefix (e.g. `"shrink:b:"`) [`build_pagination_keyboard`] prepends
    /// to a target page number, folding the view into the prefix itself.
    pub(crate) fn callback_prefix(self) -> String {
        format!("{}:{self}:", ShrinkCallbackData::prefix())
    }
}

/// Callback payload of the "prev/next page" buttons on a shrink list. Wire format:
/// `shrink:<view>:<page>`.
#[derive(Display)]
#[display("{view}:{page}")]
pub(crate) struct ShrinkCallbackData {
    pub view: ShrinkView,
    pub page: Page,
}

impl CallbackDataWithPrefix for ShrinkCallbackData {
    fn prefix() -> &'static str {
        "shrink"
    }
}

impl TryFrom<String> for ShrinkCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        let mut parts = data.as_str().split(':');
        let view = match parts.next().ok_or_else(|| err.missing_part("view"))? {
            "b" => ShrinkView::Broadcast,
            "r" => ShrinkView::Recent,
            _ => return Err(err.split_err()),
        };
        let page = callbacks::parse_part(&mut parts, &err, "page")?;
        Ok(Self { view, page })
    }
}

/// Fetches and renders one page of `view`'s shrink list. Shared by the broadcast, the inline
/// `shrinks` command, and their pagination callback.
pub(crate) async fn shrinks_page_impl(
    repos: &Repositories,
    config: &AppConfig,
    chat_id: &ChatIdKind,
    lang_code: &LanguageCode,
    view: ShrinkView,
    page: Page,
) -> anyhow::Result<ShrinksPage> {
    let offset = Offset::calculate(page, config.top_limit);
    let query_limit = (config.top_limit + 1)?; // fetch +1 row to know whether more rows exist or not
    let shrinks = repos.shrinks
        .get_recent_shrinks(chat_id, view.days(config), offset, query_limit)
        .await?;
    let has_more_pages = (shrinks.len() as i16) > config.top_limit.value();

    if shrinks.is_empty() {
        let lines = t!("commands.shrink.empty", locale = lang_code).to_string();
        return Ok(ShrinksPage { lines, has_more_pages: false });
    }

    let prefix = view.template_prefix();
    let line_key = format!("{prefix}.line");
    let lines = shrinks.iter()
        .take(config.top_limit.value() as usize)
        .map(|s| {
            let name = s.owner_name.escaped();
            t!(&line_key, locale = lang_code, uid = s.uid, name = name, lost = s.lost_length, length = s.length)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let header_key = format!("{prefix}.header");
    // `days` only matters for the notification header; rust-i18n silently ignores unused args,
    // so it's safe to pass it unconditionally for the recent header too.
    let header = t!(&header_key, locale = lang_code, days = config.stale_dicks_shrinking.grace_period_days);
    Ok(ShrinksPage { lines: format!("{header}\n{lines}"), has_more_pages })
}

/// Builds the inline `shrinks` command reply: shrink events of the last `shrink_events_days` days,
/// newest first, page 0. Mirrors the shape of [`crate::handlers::stats::chat_stats_impl`].
pub(crate) async fn recent_shrinks_impl(
    repos: &Repositories,
    from_refs: FromRefs<'_>,
    config: &AppConfig,
    lang_code: &LanguageCode,
) -> anyhow::Result<ShrinksPage> {
    shrinks_page_impl(repos, config, &from_refs.1.kind(), lang_code, ShrinkView::Recent, Page::first()).await
}

pub fn callback_filter(query: CallbackQuery) -> bool {
    ShrinkCallbackData::check_prefix(query)
}

pub async fn callback_handler(
    bot: Bot,
    q: CallbackQuery,
    config: AppConfig,
    repos: Repositories,
    lang_code: LanguageCode,
) -> HandlerResult {
    let edit_msg_req_params = callbacks::get_params_for_message_edit(&q)?;
    if !config.stale_dicks_shrinking.enabled() {
        return answer_callback_feature_disabled(bot, &q, edit_msg_req_params, lang_code).await
    }

    let data = ShrinkCallbackData::parse(&q).map_err(|e| anyhow!(e))?;
    let chat_id: ChatIdKind = edit_msg_req_params.clone().into();
    let page = shrinks_page_impl(&repos, &config, &chat_id, &lang_code, data.view, data.page).await?;

    let keyboard = build_pagination_keyboard(data.page, page.has_more_pages, &data.view.callback_prefix());
    let (answer_callback_query_result, edit_message_result) = match &edit_msg_req_params {
        callbacks::EditMessageReqParamsKind::Chat(chat_id, message_id) => {
            let mut edit_message_text_req = bot.edit_message_text(*chat_id, *message_id, page.lines);
            edit_message_text_req.parse_mode.replace(ParseMode::Html);
            edit_message_text_req.reply_markup.replace(keyboard);
            join(
                bot.answer_callback_query(q.id.clone()).into_future(),
                edit_message_text_req.into_future().map_ok(|_| ())
            ).await
        },
        callbacks::EditMessageReqParamsKind::Inline { inline_message_id, .. } => {
            let mut edit_message_text_inline_req = bot.edit_message_text_inline(inline_message_id, page.lines);
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
