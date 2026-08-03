mod dick;
mod help;
mod start;
mod privacy;
mod support;
mod dod;
mod import;
mod promo;
mod inline;
pub mod shrink;
pub mod language;
pub mod utils;
pub mod pvp;
pub mod perks;
pub mod loan;
pub mod stats;
pub mod setup;

use derive_more::Constructor;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::payloads::{AnswerCallbackQuerySetters, SendMessage, SendMessageSetters};
use teloxide::requests::{JsonRequest, Requester};
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{CallbackQuery, InlineKeyboardButton, InlineKeyboardMarkup, Message, ReplyParameters};
use teloxide::types::ParseMode::Html;

pub use dick::*;
pub use help::*;
pub use start::*;
pub use privacy::*;
pub use support::*;
pub use dod::*;
pub use import::*;
pub use inline::*;
pub use promo::*;
pub use language::LanguageCommands;
pub use loan::LoanCommands;
use crate::config::{AppConfig, MessageGroup};
use crate::domain::primitives::LanguageCode;
use crate::handlers::utils::callbacks::CallbackDataWithPrefix;
use crate::handlers::utils::SelfDestructionService;
use crate::repo::Repositories;
use crate::users::LanguageResolver;

const BANNED_SQL_CODE: &str = "GD3E1";
const BAN_DATE_FORMAT: &str = "%d.%m.%Y";

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
pub struct HandlerDeps {
    pub repos: Repositories,
    pub config: AppConfig,
    pub self_destruction: SelfDestructionService,
    pub lang_resolver: LanguageResolver,
}

/// A message sender's Telegram id for `#[tracing::instrument]` span fields
/// (`None` for messages without a sender, e.g. anonymous/channel posts).
pub(crate) fn msg_user_id(msg: &Message) -> Option<u64> {
    msg.from.as_ref().map(|u| u.id.0)
}

/// The chat id a callback query originated from, when its message is still accessible —
/// for `#[tracing::instrument]` span fields.
pub(crate) fn cq_chat_id(query: &CallbackQuery) -> Option<i64> {
    query.message.as_ref().map(|m| m.chat().id.0)
}

/// A reply text tagged with its self-destruction [`MessageGroup`](MessageGroup), so the caller
/// knows whether (and how soon) to schedule it for deletion — for commands whose reply may be
/// either a permanent event or an ephemeral status.
pub(crate) struct TaggedReply {
    pub text: String,
    pub group: MessageGroup,
}

pub enum CallbackResult {
    EditMessage(String, Option<InlineKeyboardMarkup>),
    ShowError(String),
}

impl CallbackResult {
    pub async fn apply(self, bot: Bot, callback_query: CallbackQuery) -> anyhow::Result<()> {
        let answer_req = bot.answer_callback_query(callback_query.id);
        match self {
            CallbackResult::EditMessage(text, keyboard) => {
                if let Some(message) = callback_query.message {
                    let mut edit_req = bot.edit_message_text(message.chat().id, message.id(), text);
                    edit_req.parse_mode.replace(Html);
                    edit_req.reply_markup = keyboard;

                    let edit_req_resp = edit_req.await;
                    if let Err(err) = edit_req_resp {
                        tracing::error!(chat_id = %message.chat().id, message_id = %message.id(), error = %err,
                            "couldn't edit the message");
                        Err(err)?;
                    }
                } else if let Some(inline_message_id) = callback_query.inline_message_id {
                    let mut edit_req = bot.edit_message_text_inline(&inline_message_id, text);
                    edit_req.parse_mode.replace(Html);
                    edit_req.reply_markup = keyboard;

                    let edit_req_resp = edit_req.await;
                    if let Err(err) = edit_req_resp {
                        tracing::error!(inline_message_id = %inline_message_id, error = %err,
                            "couldn't edit the inline message");
                        Err(err)?;
                    }
                };
                answer_req.await?;
            },
            CallbackResult::ShowError(err) => {
                answer_req
                    .text(err)
                    .show_alert(true)
                    .await?;
            }
        };
        Ok(())
    }
}

pub enum HandlerImplResult<D: CallbackDataWithPrefix> {
    WithKeyboard {
        text: String,
        buttons: Vec<CallbackButton<D>>
    },
    OnlyText(String)
}

#[derive(Constructor)]
pub struct CallbackButton<D: CallbackDataWithPrefix> {
    title: String,
    data: D,
}

impl <D: CallbackDataWithPrefix> HandlerImplResult<D> {
    pub fn text(&self) -> String {
        match self {
            HandlerImplResult::WithKeyboard { text, .. } => text,
            HandlerImplResult::OnlyText(text) => text
        }.clone()
    }

    pub fn keyboard(&self) -> Option<InlineKeyboardMarkup> {
        match self {
            HandlerImplResult::WithKeyboard { buttons, .. } => {
                let buttons = buttons.iter()
                    .map(|btn| InlineKeyboardButton::callback(btn.title.clone(), btn.data.to_data_string()));
                let keyboard = InlineKeyboardMarkup::new(vec![buttons]);
                Some(keyboard)
            }
            HandlerImplResult::OnlyText(_) => None
        }
    }
}

pub fn reply_html<T: Into<String>>(bot: Bot, msg: &Message, answer: T) -> JsonRequest<SendMessage> {
    // TODO: split to several messages if the answer is too long
    let mut answer = bot.send_message(msg.chat.id, answer)
        .parse_mode(Html)
        .disable_link_preview(true);
    if msg.chat.is_group() || msg.chat.is_supergroup() {
        answer.reply_parameters.replace(ReplyParameters::new(msg.id));
    }
    answer
}

#[macro_export]
macro_rules! reply_html {
    // `$bot` is an expr (not an ident) so call sites may pass e.g. `bot.clone()`. It is
    // used exactly once below, so there is no double-evaluation concern.
    ($bot:expr, $msg:ident, $answer:expr) => {
        anyhow::Context::context(
            reply_html($bot, &$msg, $answer).await,
            format!("failed for {:?}", $msg)
        )?
    };
}

/// Like [`reply_html!`], but additionally registers the sent message with the
/// [`SelfDestructionService`](SelfDestructionService) so it
/// self-destructs after the delay configured for `$group`. `$lang` (a `&LanguageCode`) is
/// used to localize the optional deletion warning. Evaluates to the sent `Message`.
/// `$svc` must be a `SelfDestructionService` in scope.
#[macro_export]
macro_rules! reply_html_ephemeral {
    ($bot:ident, $msg:ident, $answer:expr, $svc:ident, $group:expr, $lang:expr) => {{
        // Send with a clone (`reply_html` consumes the `Bot`) and keep `$bot` for the
        // scheduler (a `Bot` is a cheap `Arc` clone).
        let sent = $crate::reply_html!($bot.clone(), $msg, $answer);
        $svc.schedule(&$bot, &sent, $group, $lang);
        sent
    }};
}

pub async fn send_error_callback_answer(bot: Bot, query: CallbackQuery, tr_key: &str) -> HandlerResult {
    let lang_code = LanguageCode::from_user(&query.from);
    bot.answer_callback_query(query.id)
        .show_alert(true)
        .text(t!(tr_key, locale = &lang_code))
        .await?;
    Ok(())
}

/// Answers a paging callback query with the "feature disabled" alert and strips the now-useless
/// keyboard off the message it was attached to — a stale button can outlive the toggle that
/// enabled its feature.
pub(crate) async fn answer_callback_feature_disabled(
    bot: Bot,
    q: &CallbackQuery,
    edit_msg_req_params: utils::callbacks::EditMessageReqParamsKind,
    lang_code: LanguageCode,
) -> HandlerResult {
    let mut answer = bot.answer_callback_query(q.id.clone());
    answer.show_alert.replace(true);
    answer.text.replace(t!("errors.feature_disabled", locale = &lang_code).to_string());
    answer.await?;

    match edit_msg_req_params {
        utils::callbacks::EditMessageReqParamsKind::Chat(chat_id, message_id) =>
            bot.edit_message_reply_markup(chat_id, message_id)
                .await.map(|_| ())?,
        utils::callbacks::EditMessageReqParamsKind::Inline { inline_message_id, .. } =>
            bot.edit_message_reply_markup_inline(inline_message_id)
                .await.map(|_| ())?
    };
    Ok(())
}

pub(crate) fn banned_until_of(e: &anyhow::Error) -> Option<String> {
    e.downcast_ref::<sqlx::Error>()
     .and_then(sqlx::Error::as_database_error)
     .filter(|e| e.code().is_some_and(|code| code == BANNED_SQL_CODE))
     .map(|e| e.message().to_owned())
}

pub mod checks {
    use autometrics::autometrics;
    use std::ops::Not;
    use chrono::{DateTime, Utc};
    use rust_i18n::t;
    use teloxide::Bot;
    use teloxide::dispatching::{HandlerExt, UpdateFilterExt, UpdateHandler};
    use teloxide::payloads::{AnswerCallbackQuerySetters, AnswerInlineQuerySetters};
    use teloxide::requests::Requester;
    use teloxide::types::{InlineQueryResultArticle, InputMessageContent, InputMessageContentText, Message, Update, UpdateKind};
    use teloxide::utils::command::BotCommands;
    use crate::bans::BanList;
    use crate::config::MessageGroup;
    use crate::domain::primitives::{LanguageCode, UserId};
    use crate::domain::primitives::chat::TelegramChatId;
    use crate::handlers::HandlerDeps;
    use crate::metrics;
    use super::{BAN_DATE_FORMAT, HandlerResult, reply_html};

    pub fn is_group_chat(msg: Message) -> bool {
        if msg.chat.is_private() || msg.chat.is_channel() {
            return false
        }
        true
    }

    pub fn is_not_group_chat(msg: Message) -> bool {
        !is_group_chat(msg)
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
    pub async fn handle_not_group_chat(bot: Bot, msg: Message, deps: HandlerDeps) -> HandlerResult {
        let HandlerDeps { lang_resolver, .. } = deps;
        let lang_code = lang_resolver.execute().await;
        let answer = t!("errors.not_group_chat", locale = &lang_code);
        reply_html!(bot, msg, answer);
        Ok(())
    }

    pub fn is_group_account(msg: Message) -> bool {
        // Anonymous group admins and channel senders post *on behalf of a chat*,
        // which Telegram signals via `sender_chat`. Such accounts must not play,
        // otherwise they occupy a separate leaderboard position (issues #99, #109).
        msg.sender_chat.is_some()
    }

    /// A sub-branch that rejects messages sent on behalf of a chat (see
    /// [`is_group_account`]) before they reach a command's endpoint.
    pub fn reject_group_accounts() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync>> {
        teloxide::dptree::filter(is_group_account).endpoint(handle_group_account)
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
    async fn handle_group_account(
        bot: Bot,
        msg: Message,
        deps: HandlerDeps,
    ) -> HandlerResult {
        let HandlerDeps { lang_resolver, self_destruction, .. } = deps;
        let lang_code = lang_resolver.execute().await;
        let answer = t!("errors.group_account", locale = &lang_code);
        reply_html_ephemeral!(bot, msg, answer, self_destruction, MessageGroup::Notice, &lang_code);
        Ok(())
    }

    /// A legacy (basic) group can't be identified from an inline invocation, so playing in one
    /// before its `chat_id`↔`chat_instance` pairing has been captured would silently split the
    /// chat's state in two (issue #55). This predicate spots such not-yet-anchored groups.
    ///
    /// Only relevant when `chats_merging` is on: with it off, inline and command invocations are
    /// deliberately kept in separate rows, so there is nothing to pair up and nothing to gate.
    // No `#[autometrics]`: this returns a plain `bool` and deliberately swallows a DB error into
    // `false` (fail-open), so an error counter here would read zero exactly when the gate breaks.
    #[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0))]
    async fn needs_setup(msg: Message, deps: HandlerDeps) -> bool {
        let HandlerDeps { repos, config, .. } = deps;
        if !config.features.chats_merging || !msg.chat.is_group() {
            return false
        }
        repos.chats.is_anchored(&TelegramChatId::from(msg.chat.id))
            .await
            .inspect_err(|e| tracing::error!(error = %e, "couldn't check whether the chat is anchored"))
            .map(bool::not)
            // on a DB error, let the command through rather than locking the chat out
            .unwrap_or(false)
    }

    /// A sub-branch that intercepts chat-stateful commands in legacy groups which haven't been
    /// anchored yet (see [`needs_setup`]) and asks for the one-time setup tap instead.
    ///
    /// Also covers groups the bot joined before this shipped: those emit no "added" event, so the
    /// setup message can only be triggered lazily, by the first command.
    pub fn require_anchored_group() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync>> {
        teloxide::dptree::filter_async(needs_setup).endpoint(handle_needs_setup)
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg)))]
    async fn handle_needs_setup(bot: Bot, msg: Message) -> HandlerResult {
        let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
        super::setup::send_setup_message(&bot, msg.chat.id, &lang_code).await?;
        Ok(())
    }
    
    fn banned_until(upd: &Update, ban_list: BanList) -> Option<DateTime<Utc>> {
        upd.from()
           .map(UserId::from)
           .and_then(|uid| ban_list.banned_until(uid))
    }

    #[inline]
    pub fn is_banned(upd: Update, ban_list: BanList) -> bool {
        banned_until(&upd, ban_list).is_some()
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = ?upd.from().map(|u| u.id.0), lang_code = tracing::field::Empty))]
    pub async fn handle_banned(bot: Bot, upd: Update, bans: BanList, deps: HandlerDeps) -> HandlerResult {
        let HandlerDeps { lang_resolver, self_destruction, .. } = deps;
        let Some(until) = banned_until(&upd, bans) else {
            return Ok(())
        };
        metrics::BANNED_UPDATES_BLOCKED.inc();

        let lang_code = lang_resolver.execute().await;
        let answer = t!("errors.banned", locale = &lang_code,
            date = until.format(BAN_DATE_FORMAT).to_string());

        match upd.kind {
            UpdateKind::Message(msg) => {
                reply_html_ephemeral!(bot, msg, answer, self_destruction, MessageGroup::Notice, &lang_code);
            }
            UpdateKind::CallbackQuery(query) => {
                bot.answer_callback_query(query.id)
                    .show_alert(true)
                    .text(answer)
                    .await?;
            }
            UpdateKind::InlineQuery(query) => {
                let title = t!("errors.banned_inline_title", locale = &lang_code);
                let content = InputMessageContent::Text(InputMessageContentText::new(answer));
                let article = InlineQueryResultArticle::new("banned", title, content);
                bot.answer_inline_query(query.id, vec![article.into()])
                    .is_personal(true)
                    .cache_time(1)
                    .await?;
            }
            // a chosen inline result has nothing to answer to
            _ => {}
        }
        Ok(())
    }

    /// A branch for a chat-stateful group command. They all sit behind the same checks: they only
    /// make sense in a group, never on behalf of a group account ([`is_group_account`]), and not
    /// until a legacy group has been anchored ([`require_anchored_group`]).
    ///
    /// The command filter stays innermost on purpose — hoisting the checks above it would apply
    /// them to every group message, not just to these commands.
    pub fn group_command<C>() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync>>
    where
        C: BotCommands + Send + Sync + 'static
    {
        Update::filter_message().filter_command::<C>()
            .filter(is_group_chat)
            .branch(reject_group_accounts())
            .branch(require_anchored_group())
    }

    pub mod inline {
        use autometrics::autometrics;
        use teloxide::Bot;
        use teloxide::payloads::AnswerInlineQuerySetters;
        use teloxide::prelude::{InlineQuery, Requester};
        use teloxide::types::ChatType;
        use super::HandlerResult;

        pub fn is_group_chat(query: InlineQuery) -> bool {
            query.chat_type
                .map(|t| [ChatType::Group, ChatType::Supergroup].contains(&t))
                .unwrap_or(false)
        }

        pub fn is_not_group_chat(query: InlineQuery) -> bool {
            !is_group_chat(query)
        }

        #[autometrics]
        #[tracing::instrument(skip_all, fields(uid = query.from.id.0))]
        pub async fn handle_not_group_chat_inline(bot: Bot, query: InlineQuery) -> HandlerResult {
            bot.answer_inline_query(query.id, vec![])
                .is_personal(true)
                .cache_time(1)
                .await?;
            Ok(())
        }
    }
}
