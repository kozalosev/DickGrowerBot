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
pub mod topics;
pub mod cleanup;
pub mod rights;

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
pub use topics::TopicsCommands;
pub use cleanup::CleanupCommands;
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

pub fn reply_html<T: Into<String>>(bot: &Bot, msg: &Message, answer: T) -> JsonRequest<SendMessage> {
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
    // `$bot` is an expr (not an ident) so call sites may pass whatever they hold. It is used
    // exactly once below, and only by reference, so it stays theirs afterwards.
    //
    // Trailing `field = value` pairs set the optional parts of the request. Each goes through
    // `Into`, and every one of those fields is an `Option`, so both `reply_markup = markup` (an
    // `Option` already) and `parse_mode = ParseMode::Html` (the bare value) land correctly.
    ($bot:expr, $msg:ident, $answer:expr $(, $field:ident = $value:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut request = reply_html(&$bot, &$msg, $answer);
        $( request.$field = ::core::convert::Into::into($value); )*
        // The error handler runs outside the handler's span, so the ids of the message being
        // answered survive only in this context.
        anyhow::Context::context(
            request.await,
            format!("failed to answer message {} in chat {}", $msg.id.0, $msg.chat.id.0)
        )?
    }};
}

/// Like [`reply_html!`], but additionally registers the sent message with the
/// [`SelfDestructionService`](SelfDestructionService) so it
/// self-destructs after the delay configured for `$group`, together with the command `$msg` that
/// caused it. `$lang` (a `&LanguageCode`) is used to localize the optional deletion warning.
/// Evaluates to the sent `Message`. `$svc` must be a `SelfDestructionService` in scope.
#[macro_export]
macro_rules! reply_html_ephemeral {
    ($bot:ident, $msg:ident, $answer:expr, $svc:ident, $group:expr, $lang:expr
     $(, $field:ident = $value:expr)* $(,)?) => {{
        let sent = $crate::reply_html!($bot, $msg, $answer $(, $field = $value)*);
        $svc.schedule(&$bot, &$msg, &sent, $group, &$lang).await;
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
    use teloxide::types::{CallbackQuery, InlineQueryResultArticle, InputMessageContent, InputMessageContentText, Me, Message, Update, UpdateKind};
    use teloxide::utils::command::BotCommands;
    use crate::bans::BanList;
    use crate::commands::COMMAND_NAMES;
    use crate::config::MessageGroup;
    use crate::domain::primitives::{LanguageCode, UserId};
    use crate::domain::primitives::chat::{ChatIdKind, TelegramChatId, TopicId};
    use crate::handlers::HandlerDeps;
    use crate::handlers::utils::callbacks::CallbackDataWithPrefix;
    use crate::handlers::utils::is_forum;
    use crate::metrics;
    use crate::topics::TopicPolicy;
    use super::{reply_html, topics, HandlerResult, BAN_DATE_FORMAT};

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
        reply_html_ephemeral!(bot, msg, answer, self_destruction, MessageGroup::Notice, lang_code);
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
    
    /// Whether this message is a command addressed to *us*, in a chat where topics can restrict it.
    ///
    /// Three things keep it narrow. Ordinary messages are left alone — a notice on every message of
    /// a forbidden topic would be far noisier than the bot the setting was meant to quiet. Only a
    /// real forum is gated: a supergroup linked to a channel puts a `message_thread_id` on the
    /// messages of a discussion thread too, which has nothing to do with topics. And a group tends
    /// to hold several bots, so a command has to be ours before we answer for it — by name
    /// ([`COMMAND_NAMES`]) and, when Telegram's menu appended one, by the `@username` too.
    ///
    /// `/topics` is *not* exempted here. It is a branch of its own above this gate, so its messages
    /// never reach the predicate.
    fn is_gated_by_topic(msg: &Message, me: &Me) -> bool {
        if !is_forum(&msg.chat) {
            return false
        }
        let Some(command) = msg.text().or_else(|| msg.caption())
            .and_then(|text| text.split_whitespace().next())
            .and_then(|word| word.strip_prefix('/'))
        else {
            return false
        };
        let (name, addressee) = command.split_once('@')
            .map_or((command, None), |(name, at)| (name, Some(at)));
        if addressee.is_some_and(|at| !at.eq_ignore_ascii_case(me.username())) {
            return false
        }
        COMMAND_NAMES.contains(&name.to_lowercase())
    }

    #[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, topic_id = %TopicId::from(msg.thread_id)))]
    pub async fn is_forbidden_topic(msg: Message, me: Me, topics: TopicPolicy) -> bool {
        is_gated_by_topic(&msg, &me)
            && !topics.allows(&msg.chat.id.into(), msg.thread_id.into()).await
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
    pub async fn handle_forbidden_topic(
        bot: Bot,
        msg: Message,
        deps: HandlerDeps,
    ) -> HandlerResult {
        let HandlerDeps { lang_resolver, self_destruction, .. } = deps;
        metrics::TOPIC_RESTRICTED.inc();

        let lang_code = lang_resolver.execute().await;
        let answer = t!("errors.forbidden_topic", locale = &lang_code);
        reply_html_ephemeral!(bot, msg, answer, self_destruction, MessageGroup::Notice, lang_code);
        Ok(())
    }

    /// The chat and the topic a button press must be judged by, or `None` when there is nothing to
    /// judge: the picker's own buttons (exempt for the reason `/topics` is — they are the way back
    /// out), a message too old for Telegram to still attach, or a chat that is not a forum.
    fn callback_topic(query: &CallbackQuery) -> Option<(ChatIdKind, TopicId)> {
        let is_picker = query.data.as_deref()
            .is_some_and(|data| data.starts_with(topics::TopicsCallbackData::prefix()));
        if is_picker {
            return None
        }
        let msg = query.message.as_ref()?.regular_message()?;
        is_forum(&msg.chat)
            .then(|| (msg.chat.id.into(), msg.thread_id.into()))
    }

    /// The same gate for the buttons. A keyboard outlives the message it came with, so one sent
    /// before the restriction — or in a topic dropped since — would otherwise keep working and let
    /// the whole game be played from a topic the chat keeps the bot out of.
    pub async fn is_forbidden_topic_callback(query: CallbackQuery, topics: TopicPolicy) -> bool {
        let Some((chat_id, topic)) = callback_topic(&query) else {
            return false
        };
        !topics.allows(&chat_id, topic).await
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
    pub async fn handle_forbidden_topic_callback(
        bot: Bot,
        query: CallbackQuery,
        deps: HandlerDeps,
    ) -> HandlerResult {
        let HandlerDeps { lang_resolver, .. } = deps;
        metrics::TOPIC_RESTRICTED.inc();

        let lang_code = lang_resolver.execute().await;
        bot.answer_callback_query(query.id)
            .show_alert(true)
            .text(t!("errors.forbidden_topic_callback", locale = &lang_code))
            .await?;
        Ok(())
    }

    fn banned_until(upd: &Update, ban_list: BanList) -> Option<DateTime<Utc>> {
        upd.from()
           .map(UserId::from)
           .and_then(|uid| ban_list.banned_until(uid))
    }

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
                reply_html_ephemeral!(bot, msg, answer, self_destruction, MessageGroup::Notice, lang_code);
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

    #[cfg(test)]
    mod test {
        use teloxide::types::{CallbackQuery, Me, Message};
        use crate::domain::primitives::chat::{ChatIdKind, TelegramChatId, TopicId};
        use super::{callback_topic, is_gated_by_topic};

        /// A message of the given text, in a supergroup that either is a forum or isn't. Built
        /// from JSON because a `Message` has far more fields than a test wants to name.
        fn message(text: &str, is_forum: bool, thread_id: Option<i32>) -> Message {
            let thread = thread_id
                .map(|id| format!(r#""message_thread_id":{id},"is_topic_message":true,"#))
                .unwrap_or_default();
            let json = format!(r#"{{
                "chat": {{"id":-1001847508954,"is_forum":{is_forum},"title":"chat","type":"supergroup"}},
                "date": 1675229140,
                "from": {{"first_name":"tester","id":1253681278,"is_bot":false}},
                {thread}"message_id": 5,
                "text": "{text}"
            }}"#);
            serde_json::from_str(&json).expect("couldn't build the message")
        }

        /// The bot the commands are addressed to in the tests below.
        fn me() -> Me {
            let json = r#"{
                "id": 42, "is_bot": true, "first_name": "Dick Grower", "username": "DickGrowerBot",
                "can_join_groups": true, "can_read_all_group_messages": false,
                "supports_inline_queries": true, "can_connect_to_business_account": false,
                "has_main_web_app": false
            }"#;
            serde_json::from_str(json).expect("couldn't build the bot's own user")
        }

        #[test]
        fn test_our_command_in_a_forum_is_gated() {
            assert!(is_gated_by_topic(&message("/grow", true, Some(42)), &me()));
            // the General topic carries no thread id of its own
            assert!(is_gated_by_topic(&message("/grow", true, None), &me()));
            assert!(is_gated_by_topic(&message("/grow@DickGrowerBot arg", true, Some(42)), &me()));
            // Telegram's menu is not consistent about the case of the username it appends
            assert!(is_gated_by_topic(&message("/grow@dickgrowerbot", true, Some(42)), &me()));
        }

        /// A group usually holds several bots. Answering for a command aimed at one of them would
        /// be noise from a bot that was just told to be quieter, so the addressee is checked.
        #[test]
        fn test_another_bots_command_is_not_gated() {
            assert!(!is_gated_by_topic(&message("/grow@SomeOtherBot", true, Some(42)), &me()));
            assert!(!is_gated_by_topic(&message("/start@SomeOtherBot", true, Some(42)), &me()));
        }

        /// Same reasoning for a bare command we simply don't have: it belongs to someone else.
        #[test]
        fn test_a_command_we_dont_have_is_not_gated() {
            assert!(!is_gated_by_topic(&message("/roll 2d6", true, Some(42)), &me()));
            assert!(!is_gated_by_topic(&message("/", true, Some(42)), &me()));
        }

        /// Every message of a forbidden topic would otherwise get a notice, which is noisier than
        /// the bot the setting was meant to quiet.
        #[test]
        fn test_an_ordinary_message_is_not_gated() {
            assert!(!is_gated_by_topic(&message("hello", true, Some(42)), &me()));
            assert!(!is_gated_by_topic(&message("not a /grow", true, Some(42)), &me()));
        }

        /// A supergroup linked to a channel puts a thread id on discussion-thread messages too,
        /// and those aren't topics.
        #[test]
        fn test_a_non_forum_is_never_gated() {
            assert!(!is_gated_by_topic(&message("/grow", false, Some(42)), &me()));
            assert!(!is_gated_by_topic(&message("/grow", false, None), &me()));
        }

        /// `/topics` is not exempted here: it is a branch of its own above this gate, so a
        /// `/topics` message never reaches the predicate at all. This pins that down — if the
        /// branch is ever moved below the gate, the reminder is right here.
        #[test]
        fn test_the_topics_command_is_exempted_by_the_branch_order_only() {
            assert!(is_gated_by_topic(&message("/topics", true, Some(42)), &me()));
        }

        fn callback(data: &str, message: Option<Message>) -> CallbackQuery {
            let message = message
                .map(|msg| serde_json::to_string(&msg).expect("couldn't serialize the message"))
                .unwrap_or_else(|| "null".to_owned());
            let json = format!(r#"{{
                "id": "1",
                "from": {{"first_name":"tester","id":1253681278,"is_bot":false}},
                "chat_instance": "1",
                "data": "{data}",
                "message": {message}
            }}"#);
            serde_json::from_str(&json).expect("couldn't build the callback query")
        }

        #[test]
        fn test_a_button_in_a_forum_is_judged_by_its_topic() {
            let query = callback("pvp:1:2", Some(message("/pvp", true, Some(42))));
            let topic = callback_topic(&query);
            assert_eq!(topic, Some((ChatIdKind::ID(TelegramChatId::new(-1001847508954)), TopicId::new(42))));

            // the General topic carries no thread id of its own
            let query = callback("pvp:1:2", Some(message("/pvp", true, None)));
            let topic = callback_topic(&query).map(|(_, topic)| topic);
            assert_eq!(topic, Some(TopicId::GENERAL));
        }

        /// The picker's buttons are the way back out of a restriction, so they are never judged
        /// by it — pressing one inside a forbidden topic has to keep working.
        #[test]
        fn test_the_pickers_own_buttons_are_never_judged() {
            let query = callback("topics:1:0:all", Some(message("/topics", true, Some(42))));
            assert_eq!(callback_topic(&query), None);
        }

        #[test]
        fn test_a_button_outside_a_forum_is_never_judged() {
            let query = callback("pvp:1:2", Some(message("/pvp", false, Some(42))));
            assert_eq!(callback_topic(&query), None);
        }

        /// An inline message, or one too old for Telegram to attach, leaves nothing to judge by —
        /// no chat kind and no topic — so the gate has to let it through rather than guess.
        #[test]
        fn test_a_button_without_a_message_is_never_judged() {
            let query = callback("pvp:1:2", None);
            assert_eq!(callback_topic(&query), None);
        }
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
