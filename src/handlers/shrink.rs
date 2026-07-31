
use anyhow::anyhow;
use chrono::NaiveDate;
use derive_more::Display;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup};
use crate::config::AppConfig;
use crate::domain::primitives::{LanguageCode, Length, Offset, Page, UserId, Username};
use crate::domain::primitives::chat::ChatIdKind;
use crate::handlers::{answer_callback_feature_disabled, FromRefs, HandlerDeps, HandlerResult};
use crate::handlers::utils::callbacks;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::repo::{AdjacentDates, RecentShrink, Repositories, ShrinkEvent};

pub(crate) struct ShrinksPage {
    pub lines: String,
    pub(crate) has_more_pages: bool,
}

#[derive(Display, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShrinkView {
    /// The proactive midnight broadcast — pinned to the exact day it was sent, forever. Never
    /// shows day-navigation: a notification is about one day by definition.
    #[display("b")]
    Broadcast,
    /// The inline `shrinks` command — opens on the most recent date with data and can page along
    /// both axes: users (within a date) and days (unbounded — jumps to whichever adjacent date
    /// actually has data, however far back that is).
    #[display("r")]
    Recent,
}

impl ShrinkView {
    fn template_prefix(self) -> &'static str {
        match self {
            ShrinkView::Broadcast => "commands.shrink.notification",
            ShrinkView::Recent => "commands.shrink.recent",
        }
    }
}

/// Lets a page render from either a fresh [`RecentShrink`] query or the [`ShrinkEvent`]s the daily
/// job already holds, so the broadcast's page 0 needs no query at all.
pub(crate) trait ShrinkLine {
    fn uid(&self) -> UserId;
    fn owner_name(&self) -> &Username;
    fn lost_length(&self) -> Length;
    fn length(&self) -> Length;
}

impl ShrinkLine for RecentShrink {
    fn uid(&self) -> UserId { self.uid }
    fn owner_name(&self) -> &Username { &self.owner_name }
    fn lost_length(&self) -> Length { self.lost_length }
    fn length(&self) -> Length { self.length }
}

impl ShrinkLine for ShrinkEvent {
    fn uid(&self) -> UserId { self.uid }
    fn owner_name(&self) -> &Username { &self.owner_name }
    fn lost_length(&self) -> Length { self.lost_length }
    fn length(&self) -> Length { self.new_length }
}

/// Callback payload of a shrink list's "prev/next page" and "prev/next day" buttons. Wire format:
/// `shrink:<b|r>:<yyyy-mm-dd>:<page>`. Carrying the date (rather than a relative day window) is
/// what makes the broadcast immune to drift: a notification's buttons always re-render the day it
/// was sent, no matter when they're clicked.
#[derive(Display, Clone, Copy)]
#[display("{view}:{date}:{page}")]
pub(crate) struct ShrinkCallbackData {
    pub view: ShrinkView,
    pub date: NaiveDate,
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
        let date = callbacks::parse_part(&mut parts, &err, "date")?;
        let page = callbacks::parse_part(&mut parts, &err, "page")?;
        Ok(Self { view, date, page })
    }
}

pub(crate) fn render_shrinks_page<T: ShrinkLine>(
    rows: &[T],
    config: &AppConfig,
    lang_code: &LanguageCode,
    view: ShrinkView,
    has_more_pages: bool,
) -> ShrinksPage {
    if rows.is_empty() {
        let lines = t!("commands.shrink.empty", locale = lang_code).to_string();
        return ShrinksPage { lines, has_more_pages: false };
    }

    let prefix = view.template_prefix();
    let line_key = format!("{prefix}.line");
    let lines = rows.iter()
        .take(config.top_limit.value() as usize)
        .map(|s| {
            let name = s.owner_name().escaped();
            t!(&line_key, locale = lang_code, uid = s.uid(), name = name, lost = s.lost_length(), length = s.length())
        })
        .collect::<Vec<_>>()
        .join("\n");
    let header_key = format!("{prefix}.header");
    let header = t!(&header_key, locale = lang_code, days = config.daily_shrink.inactivity_days);
    ShrinksPage { lines: format!("{header}\n{lines}"), has_more_pages }
}

pub(crate) async fn shrinks_page_impl(
    repos: &Repositories,
    config: &AppConfig,
    chat_id: &ChatIdKind,
    lang_code: &LanguageCode,
    view: ShrinkView,
    date: NaiveDate,
    page: Page,
) -> anyhow::Result<ShrinksPage> {
    let offset = Offset::calculate(page, config.top_limit);
    let query_limit = (config.top_limit + 1)?; // fetch +1 row to know whether more rows exist or not
    let shrinks = repos.shrinks
        .get_shrinks_for_date(chat_id, date, offset, query_limit)
        .await?;
    let has_more_pages = shrinks.len() > config.top_limit.value() as usize;
    Ok(render_shrinks_page(&shrinks, config, lang_code, view, has_more_pages))
}

/// `None` when no button would do anything, so callers never attach an empty or dead row.
pub(crate) fn build_shrink_keyboard(
    view: ShrinkView,
    date: NaiveDate,
    page: Page,
    has_more_pages: bool,
    adjacent: Option<AdjacentDates>,
) -> Option<InlineKeyboardMarkup> {
    let callback = |date: NaiveDate, page: Page| ShrinkCallbackData { view, date, page }.to_data_string();

    let mut user_row = Vec::new();
    if page > 0 {
        let prev_page = (page - 1).expect("the page is positive here, so the previous one is valid");
        user_row.push(InlineKeyboardButton::callback("⬅️", callback(date, prev_page)));
    }
    if has_more_pages {
        let next_page = (page + 1).expect("the page increment saturates, so it always stays valid");
        user_row.push(InlineKeyboardButton::callback("➡️", callback(date, next_page)));
    }

    let day_row = adjacent.map(|AdjacentDates { older, newer }| {
        let mut row = Vec::new();
        if let Some(d) = older {
            row.push(InlineKeyboardButton::callback(format!("⬅️ {}", d.format("%d %b")), callback(d, Page::first())));
        }
        if let Some(d) = newer {
            row.push(InlineKeyboardButton::callback(format!("{} ➡️", d.format("%d %b")), callback(d, Page::first())));
        }
        row
    }).unwrap_or_default();

    let rows: Vec<_> = [user_row, day_row].into_iter().filter(|row| !row.is_empty()).collect();
    (!rows.is_empty()).then(|| InlineKeyboardMarkup::new(rows))
}

/// Opens on the most recent date that has shrinks.
pub(crate) async fn recent_shrinks_impl(
    repos: &Repositories,
    from_refs: FromRefs<'_>,
    config: &AppConfig,
    lang_code: &LanguageCode,
) -> anyhow::Result<(ShrinksPage, Option<InlineKeyboardMarkup>)> {
    let chat_id = from_refs.1.kind();
    let Some(date) = repos.shrinks.get_latest_shrink_date(&chat_id).await? else {
        let lines = t!("commands.shrink.empty", locale = lang_code).to_string();
        return Ok((ShrinksPage { lines, has_more_pages: false }, None));
    };

    let page = shrinks_page_impl(repos, config, &chat_id, lang_code, ShrinkView::Recent, date, Page::first()).await?;
    let adjacent = repos.shrinks.get_adjacent_shrink_dates(&chat_id, date).await?;
    let keyboard = build_shrink_keyboard(ShrinkView::Recent, date, Page::first(), page.has_more_pages, Some(adjacent));
    Ok((page, keyboard))
}

pub fn callback_filter(query: CallbackQuery) -> bool {
    ShrinkCallbackData::check_prefix(query)
}

pub async fn callback_handler(
    bot: Bot,
    q: CallbackQuery,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, config, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let edit_msg_req_params = callbacks::get_params_for_message_edit(&q)?;
    if !config.daily_shrink.enabled() {
        return answer_callback_feature_disabled(bot, &q, edit_msg_req_params, lang_code).await
    }

    let data = ShrinkCallbackData::parse(&q).map_err(|e| anyhow!(e))?;
    let chat_id: ChatIdKind = edit_msg_req_params.clone().into();
    let page = shrinks_page_impl(&repos, &config, &chat_id, &lang_code, data.view, data.date, data.page).await?;

    let adjacent = match data.view {
        ShrinkView::Recent => Some(repos.shrinks.get_adjacent_shrink_dates(&chat_id, data.date).await?),
        ShrinkView::Broadcast => None,
    };
    let keyboard = build_shrink_keyboard(data.view, data.date, data.page, page.has_more_pages, adjacent)
        .unwrap_or_default(); // an explicit empty markup clears any now-dead buttons already on the message

    callbacks::answer_and_edit_page(&bot, &q, &edit_msg_req_params, page.lines, keyboard).await?;
    Ok(())
}
