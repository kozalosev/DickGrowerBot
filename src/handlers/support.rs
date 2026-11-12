//! A contact channel that doesn't expose an email address or a personal account: the bot relays
//! the message to the chat set in `SUPPORT_CHAT_ID`, a reply to it there back to the user, and the
//! user's answer to that reply into the same thread.
//!
//! It is the way out for a data deletion or access request — see the privacy policy — so it stays
//! above the ban gate in the dispatcher tree and must never write a row for its sender.

use std::borrow::Cow;
use rustc_hash::FxHashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use autometrics::autometrics;
use derive_more::Display;
use rust_i18n::t;
use teloxide::{Bot, RequestError};
use crate::metrics::{RejectableOutcome, RejectionReason};
use crate::dialogue::CachedDialogueStorage;
use teloxide::macros::BotCommands;
use teloxide::payloads::{CopyMessageSetters, SendMessageSetters};
use teloxide::prelude::{Dialogue, Requester};
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardButtonKind, InlineKeyboardMarkup, Me,
                      Message, MessageEntityKind, MessageEntityRef, MessageId, ParseMode, ReplyParameters,
                      User as TelegramUser, UserId as TelegramUserId};
use teloxide::utils::html;
use crate::domain::primitives::{LanguageCode, UserId};
use crate::domain::primitives::chat::TelegramChatId;
use crate::handlers::{HandlerDeps, HandlerResult, reply_html, rights};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::handlers::utils::get_full_name;
use crate::{metrics, reply_html};
use crate::metrics::StartupCheck;

/// One request per user per minute. A `const` rather than an environment variable: the knob is too
/// small to be worth a line in every deployment file.
const RATE_LIMIT: Duration = Duration::from_mins(1);
const TICKET_PREFIX: &str = "🆘 ";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum SupportCommands {
    #[command(description = "support")]
    Support(String),
}

#[derive(Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SupportCommandState {
    #[default]
    Start,
    Requested,
    Replying(SupportTicket),
}

pub type SupportDialogue = Dialogue<SupportCommandState, CachedDialogueStorage<SupportCommandState>>;

/// A message of the bot in the support chat, which a user's answer is threaded under.
#[derive(Clone, Copy, Debug, Display, PartialEq, serde::Serialize, serde::Deserialize)]
#[display("{}", _0.0)]
pub struct SupportTicket(MessageId);

impl CallbackDataWithPrefix for SupportTicket {
    fn prefix() -> &'static str {
        "support"
    }
}

impl TryFrom<String> for SupportTicket {
    type Error = InvalidCallbackData;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
            .map(|id| Self(MessageId(id)))
            .map_err(|e| InvalidCallbackDataBuilder(&value).parsing_err(e))
    }
}

/// The user a reply in the support chat is meant for, and the ticket it answers.
#[derive(Clone)]
pub struct SupportRecipient {
    chat_id: ChatId,
    lang_code: LanguageCode,
    ticket: SupportTicket,
}

#[derive(Clone)]
pub struct SupportService {
    chat_id: Option<TelegramChatId>,
    last_sent: Arc<Mutex<FxHashMap<UserId, Instant>>>,
}

impl SupportService {
    pub fn new(chat_id: Option<TelegramChatId>) -> Self {
        Self { chat_id, last_sent: Default::default() }
    }

    fn extract_reply_recipient(&self, msg: &Message, bot_id: TelegramUserId) -> Option<SupportRecipient> {
        Checks::new("support reply", msg)
            .ensure(|msg| self.chat_id == Some(msg.chat.id.into()), "not the support chat")
            .ensure(|msg| !sent_by(msg, bot_id), "the bot's own message")
            .then(|msg| msg.reply_to_message(), "not a reply")
            .ensure(|original| sent_by(original, bot_id), "not a reply to the bot")
            .ensure(|original| original.chat.id == msg.chat.id, "a reply to another chat")
            .then(ticket_recipient, "not a reply to a ticket")
            .result()
    }

    async fn relay(
        &self,
        bot: &Bot,
        from: &TelegramUser,
        lang_code: &LanguageCode,
        text: &str,
        ticket: Option<SupportTicket>,
    ) -> anyhow::Result<Relayed> {
        let Some(chat_id) = self.chat_id else {
            return Ok(Relayed::Disabled)
        };
        let uid = UserId::from(from);
        if !self.pass_rate_limit(uid) {
            return Ok(Relayed::TooOften)
        }

        let name = get_full_name(from);
        // The owner reads this one, so it isn't localized.
        let message = format!(
            "{TICKET_PREFIX}<a href=\"tg://user?id={uid}\">{name}</a>\n<code>{uid}</code> · {lang_code}\n\n{text}",
            name = name.escaped(),
            text = html::escape(text),
        );
        let mut request = bot.send_message(ChatId::from(chat_id), message)
            .parse_mode(ParseMode::Html)
            .disable_link_preview(true);
        if let Some(SupportTicket(id)) = ticket {
            request = request.reply_parameters(ReplyParameters::new(id).allow_sending_without_reply());
        }
        request.await?;
        Ok(Relayed::Sent)
    }

    fn pass_rate_limit(&self, uid: UserId) -> bool {
        let now = Instant::now();
        let mut last_sent = self.last_sent.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        last_sent.retain(|_, at| now.duration_since(*at) < RATE_LIMIT);
        last_sent.insert(uid, now).is_none()
    }
}

/// Why the chat set in `SUPPORT_CHAT_ID` can't be used.
#[derive(Debug, Display)]
pub enum Unusable {
    #[display("the group was upgraded to the supergroup {}", _0.0)]
    Migrated(ChatId),
    #[display("the bot can't post there")]
    CannotPost,
    #[display("{_0}")]
    Unknown(RequestError),
}

impl From<RequestError> for Unusable {
    fn from(e: RequestError) -> Self {
        match e {
            RequestError::MigrateToChatId(new_id) => Self::Migrated(new_id),
            e => Self::Unknown(e),
        }
    }
}

/// The support chat to use: the configured one when the bot can post there, none otherwise.
pub async fn check_support_chat(bot: &Bot, chat_id: Option<TelegramChatId>, me: &Me) -> Option<TelegramChatId> {
    let chat_id = chat_id?;
    let checked = support_chat_usable(bot, chat_id, me).await;
    metrics::STARTUP_VERIFICATION_ERROR.report(StartupCheck::SupportChat, checked.is_err());
    checked
        .inspect(|()| tracing::info!(%chat_id, "the support chat is usable"))
        .inspect_err(|e| tracing::error!(%chat_id, reason = %e, "the support chat is unusable, so /support is disabled"))
        .ok()
        .map(|()| chat_id)
}

async fn support_chat_usable(bot: &Bot, chat_id: TelegramChatId, me: &Me) -> Result<(), Unusable> {
    let chat_id = ChatId::from(chat_id);
    bot.get_chat(chat_id).await?;
    if chat_id.is_user() {
        return Ok(())
    }
    let member = bot.get_chat_member(chat_id, me.id).await?;
    rights::may_post(&member)
        .then_some(())
        .ok_or(Unusable::CannotPost)
}

enum Relayed {
    Sent,
    TooOften,
    Disabled,
}

impl RejectableOutcome for Relayed {
    fn rejection(&self) -> Option<RejectionReason> {
        match self {
            Self::Sent => None,
            Self::TooOften => Some(RejectionReason::TooOften),
            Self::Disabled => Some(RejectionReason::FeatureDisabled),
        }
    }
}

impl Relayed {
    fn tr_key(&self) -> &'static str {
        match self {
            Relayed::Sent => "commands.support.sent",
            Relayed::TooOften => "commands.support.too_often",
            Relayed::Disabled => "errors.feature_disabled",
        }
    }
}

/// A chain of checks over a message that stops at the first failing one and says which it was.
struct Checks<'m, T> {
    filter: &'static str,
    msg: &'m Message,
    subject: Option<T>,
}

impl<'m> Checks<'m, &'m Message> {
    fn new(filter: &'static str, msg: &'m Message) -> Self {
        Self { filter, msg, subject: Some(msg) }
    }
}

impl<'m, T> Checks<'m, T> {
    fn ensure(self, check: impl FnOnce(&T) -> bool, reason: &'static str) -> Self {
        self.then(|subject| check(&subject).then_some(subject), reason)
    }

    fn then<U>(self, step: impl FnOnce(T) -> Option<U>, reason: &'static str) -> Checks<'m, U> {
        let Self { filter, msg, subject } = self;
        let subject = subject.and_then(|subject| step(subject)
            .or_else(|| {
                tracing::trace!(filter, reason, chat_id = msg.chat.id.0, message_id = msg.id.0, "filtered out");
                None
            }));
        Checks { filter, msg, subject }
    }

    fn result(self) -> Option<T> {
        self.subject
    }
}

pub fn support_reply_recipient(msg: Message, support: SupportService, me: Me) -> Option<SupportRecipient> {
    support.extract_reply_recipient(&msg, me.user.id)
}

/// The ticket a user's message answers, when it is a reply to an answer the bot delivered.
pub fn support_answered_ticket(msg: Message, me: Me) -> Option<SupportTicket> {
    Checks::new("support answer", &msg)
        .ensure(|msg| msg.chat.is_private(), "not a private chat")
        .then(|msg| msg.text(), "not a text message")
        .ensure(|text| !text.starts_with('/'), "a command")
        .then(|_| msg.reply_to_message(), "not a reply")
        .ensure(|answer| sent_by(answer, me.user.id), "not a reply to the bot")
        .then(|answer| answer.reply_markup(), "no keyboard")
        .then(|markup| markup.inline_keyboard.iter()
            .flatten()
            .find_map(|button| match &button.kind {
                InlineKeyboardButtonKind::CallbackData(data) => SupportTicket::from_data_string(data).ok(),
                _ => None,
            }), "no support button")
        .result()
}

pub fn support_callback_filter(query: CallbackQuery) -> bool {
    SupportTicket::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg)))]
pub async fn support_reply_handler(bot: Bot, msg: Message, recipient: SupportRecipient) -> HandlerResult {
    let SupportRecipient { chat_id, lang_code, ticket } = recipient;
    let keyboard = InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
        t!("commands.support.reply_button", locale = &lang_code),
        ticket.to_data_string(),
    )]]);
    let copied = bot.copy_message(chat_id, msg.chat.id, msg.id)
        .reply_markup(keyboard)
        .await;
    if let Err(error) = copied {
        tracing::warn!(recipient = chat_id.0, error = %error, "couldn't deliver support reply");
        bot.send_message(msg.chat.id, format!("Не удалось доставить ответ пользователю: {error}"))
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
    }
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(uid = %query.from.id))]
pub async fn support_callback_handler(
    bot: Bot,
    query: CallbackQuery,
    dialogue: SupportDialogue,
    deps: HandlerDeps,
) -> HandlerResult {
    let ticket = SupportTicket::parse(&query)?;
    let lang_code = deps.lang_resolver.execute().await;
    dialogue.update(SupportCommandState::Replying(ticket)).await?;
    bot.answer_callback_query(query.id).await?;
    bot.send_message(dialogue.chat_id(), t!("commands.support.request", locale = &lang_code))
        .await?;
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn support_cmd_handler(
    bot: Bot,
    msg: Message,
    cmd: SupportCommands,
    dialogue: SupportDialogue,
    support: SupportService,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_SUPPORT_COUNTER.inc();

    let SupportCommands::Support(text) = cmd;
    let answer = if text.trim().is_empty() {
        dialogue.update(SupportCommandState::Requested).await?;
        t!("commands.support.request", locale = &lang_code)
    } else {
        dialogue.exit().await?;
        relay(&bot, &msg, &support, &text, &lang_code, None).await?
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn support_requested_handler(
    bot: Bot,
    msg: Message,
    dialogue: SupportDialogue,
    support: SupportService,
    deps: HandlerDeps,
) -> HandlerResult {
    answer_request(bot, msg, dialogue, support, deps, None).await
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn support_follow_up_handler(
    bot: Bot,
    msg: Message,
    ticket: SupportTicket,
    dialogue: SupportDialogue,
    support: SupportService,
    deps: HandlerDeps,
) -> HandlerResult {
    answer_request(bot, msg, dialogue, support, deps, Some(ticket)).await
}

async fn answer_request(
    bot: Bot,
    msg: Message,
    dialogue: SupportDialogue,
    support: SupportService,
    deps: HandlerDeps,
    ticket: Option<SupportTicket>,
) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;

    let answer = match msg.text() {
        // Another command means the user changed their mind; sending it to the owner as the body
        // of a request would only confuse both sides.
        Some(text) if text.starts_with('/') => {
            dialogue.exit().await?;
            t!("commands.support.cancelled", locale = &lang_code)
        }
        Some(text) => {
            dialogue.exit().await?;
            relay(&bot, &msg, &support, text, &lang_code, ticket).await?
        }
        None => t!("commands.support.request", locale = &lang_code)
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

async fn relay(
    bot: &Bot,
    msg: &Message,
    support: &SupportService,
    text: &str,
    lang_code: &LanguageCode,
    ticket: Option<SupportTicket>,
) -> anyhow::Result<Cow<'static, str>> {
    let from = msg.from.as_ref().ok_or(anyhow::anyhow!("no from user"))?;
    let relayed = support.relay(bot, from, lang_code, text, ticket).await?;
    metrics::CMD_SUPPORT_REJECTED.count(&relayed);
    Ok(t!(relayed.tr_key(), locale = lang_code))
}

/// The link right after the emoji can only be the bot's own, and so can the `<code>` with the id:
/// the name and the text are escaped. The language follows that `<code>`.
fn ticket_recipient(ticket: &Message) -> Option<SupportRecipient> {
    Checks::new("ticket", ticket)
        .then(|ticket| ticket.text(), "not a text message")
        .ensure(|text| text.starts_with(TICKET_PREFIX), "no emoji in front")
        .then(|text| ticket.parse_entities().map(|entities| (text, entities)), "no entities")
        .then(|(text, entities)| author_id(&entities).map(|uid| SupportRecipient {
            chat_id: uid.into(),
            lang_code: author_language(text, &entities, uid),
            ticket: SupportTicket(ticket.id),
        }), "no link to the author after the emoji")
        .result()
}

fn author_id(entities: &[MessageEntityRef]) -> Option<TelegramUserId> {
    let link = entities.iter()
        .find(|entity| matches!(entity.kind(),
            MessageEntityKind::TextLink { .. } | MessageEntityKind::TextMention { .. }))
        .filter(|link| link.start() == TICKET_PREFIX.len())?;
    let uid = match link.kind() {
        MessageEntityKind::TextLink { url } => url.as_str()
            .strip_prefix("tg://user?id=")
            .and_then(|uid| uid.parse().ok())
            .map(TelegramUserId),
        MessageEntityKind::TextMention { user } => Some(user.id),
        _ => None,
    };
    uid.filter(|uid| uid.0 != 0)
}

fn author_language(text: &str, entities: &[MessageEntityRef], uid: TelegramUserId) -> LanguageCode {
    let uid = uid.to_string();
    entities.iter()
        .find(|entity| matches!(entity.kind(), MessageEntityKind::Code) && entity.text() == uid)
        .and_then(|code| text[code.end()..].strip_prefix(" · "))
        .and_then(|rest| rest.split('\n').next())
        .map(|code| LanguageCode::new(code.to_owned()))
        .unwrap_or_default()
}

fn sent_by(msg: &Message, user_id: TelegramUserId) -> bool {
    msg.from.as_ref()
        .is_some_and(|from| from.id == user_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use crate::fake_telegram::FakeApi;

    const SUPPORT_CHAT: i64 = -1001234567890;
    const BOT_ID: u64 = 777;
    const USER_ID: i64 = 123;
    const TICKET_ID: i32 = 10;

    #[test]
    fn routes_text_and_photo_replies_to_the_user() {
        let message = reply_message();
        let to = recipient_chat(message.clone(), Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));

        let mut mention = message.clone();
        mention["reply_to_message"]["entities"][0] = json!({
            "type": "text_mention", "offset": 3, "length": 4,
            "user": { "id": USER_ID, "is_bot": false, "first_name": "Name" }
        });
        let to = recipient_chat(mention, Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));

        let mut photo = message;
        photo.as_object_mut().unwrap().remove("text");
        photo["photo"] = json!([{
            "file_id": "photo", "file_unique_id": "unique", "width": 1, "height": 1
        }]);
        let to = recipient_chat(photo, Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));
    }

    #[test]
    fn ignores_unrelated_or_untrusted_replies() {
        let message = reply_message();
        let to = recipient_chat(message.clone(), None);
        assert_eq!(to, None);
        let to = recipient_chat(message.clone(), Some(-1009876));
        assert_eq!(to, None);

        let mut unrelated = message.clone();
        unrelated.as_object_mut().unwrap().remove("reply_to_message");
        let to = recipient_chat(unrelated, Some(SUPPORT_CHAT));
        assert_eq!(to, None);

        let mut other_sender = message.clone();
        other_sender["reply_to_message"]["from"]["id"] = json!(999);
        let to = recipient_chat(other_sender, Some(SUPPORT_CHAT));
        assert_eq!(to, None);

        let mut forged_name = message;
        forged_name["reply_to_message"]["text"] = json!("🆘 Name\n999 · en\n\nQuestion");
        let to = recipient_chat(forged_name, Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));
    }

    #[test]
    fn reads_the_ticket_and_the_language_of_its_author() {
        let mut message = reply_message();
        message["reply_to_message"]["text"] = json!("🆘 Name\n123 · ru\n\nQuestion");

        let recipient = recipient(message, Some(SUPPORT_CHAT))
            .expect("a recipient");
        assert_eq!(recipient.ticket, SupportTicket(MessageId(TICKET_ID)));
        assert_eq!(recipient.lang_code, LanguageCode::new("ru".to_owned()));
    }

    #[test]
    fn falls_back_to_the_default_language_without_the_code_line() {
        let mut message = reply_message();
        message["reply_to_message"]["entities"].as_array_mut().unwrap().pop();

        let recipient = recipient(message, Some(SUPPORT_CHAT))
            .expect("a recipient");
        assert_eq!(recipient.lang_code, LanguageCode::default());
    }

    #[test]
    fn a_ticket_survives_its_callback_data() {
        let ticket = SupportTicket(MessageId(TICKET_ID));
        let data = ticket.to_data_string();
        assert_eq!(data, "support:10");

        let parsed = SupportTicket::from_data_string(&data).ok();
        assert_eq!(parsed, Some(ticket));
        let parsed = SupportTicket::from_data_string("shrink:10").ok();
        assert_eq!(parsed, None);
    }

    #[test]
    fn a_replying_dialogue_survives_the_store() {
        let state = SupportCommandState::Replying(SupportTicket(MessageId(TICKET_ID)));
        let stored = serde_json::to_string(&state)
            .expect("couldn't serialize the state");
        let restored: SupportCommandState = serde_json::from_str(&stored)
            .expect("couldn't deserialize the state");
        assert!(restored == state);
    }

    #[test]
    fn finds_the_ticket_a_private_reply_answers() {
        struct TestCase {
            case: &'static str,
            change: fn(&mut Value),
            expected_result: Option<SupportTicket>,
        }

        impl TestCase {
            fn new(case: &'static str, change: fn(&mut Value), expected_result: SupportTicket) -> Self {
                TestCase { case, change, expected_result: Some(expected_result) }
            }

            fn with_none_result(case: &'static str, change: fn(&mut Value)) -> Self {
                TestCase { case, change, expected_result: None }
            }
        }

        let cases: [TestCase; 5] = [
            TestCase::new("an answer", |_| {}, SupportTicket(MessageId(TICKET_ID))),
            TestCase::with_none_result("a command", |msg| msg["text"] = json!("/help")),
            TestCase::with_none_result("in a group", |msg| {
                msg["chat"] = json!({ "id": SUPPORT_CHAT, "type": "supergroup", "title": "Support" });
            }),
            TestCase::with_none_result("no button", |msg| {
                msg["reply_to_message"].as_object_mut().unwrap().remove("reply_markup");
            }),
            TestCase::with_none_result("someone else's message", |msg| {
                msg["reply_to_message"]["from"]["id"] = json!(999);
            }),
        ];
        for TestCase { case, change, expected_result } in cases {
            let mut message = answer_reply_message();
            change(&mut message);
            let ticket = answered_ticket(message);
            assert_eq!(ticket, expected_result, "{case}");
        }
    }

    #[tokio::test]
    async fn delivers_the_reply_with_a_button_to_answer_it() {
        let api = fake_api(true).await;
        let msg: Message = serde_json::from_value(reply_message()).unwrap();
        let recipient = recipient(reply_message(), Some(SUPPORT_CHAT))
            .expect("a recipient");

        support_reply_handler(api.bot.clone(), msg, recipient)
            .await.unwrap();

        let calls = api.calls();
        assert_eq!(calls.len(), 1);
        let (path, body) = &calls[0];
        assert_eq!(path, "/bot123:token/CopyMessage");
        let data = &body["reply_markup"]["inline_keyboard"][0][0]["callback_data"];
        assert_eq!(data, "support:10");
    }

    #[tokio::test]
    async fn reports_a_copy_failure_in_the_support_chat() {
        let api = fake_api(false).await;
        let msg: Message = serde_json::from_value(reply_message()).unwrap();
        let recipient = recipient(reply_message(), Some(SUPPORT_CHAT))
            .expect("a recipient");

        support_reply_handler(api.bot.clone(), msg, recipient)
            .await.unwrap();

        assert_eq!(api.methods(), ["CopyMessage", "SendMessage"]);
    }

    #[tokio::test]
    async fn a_group_the_bot_may_post_in_is_usable() {
        let api = chat_api(chat_answer(SUPPORT_CHAT, "supergroup"), member_answer("member")).await;
        let result = support_chat_usable(&api.bot, TelegramChatId::new(SUPPORT_CHAT), &me()).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(api.methods(), ["GetChat", "GetChatMember"]);
    }

    #[tokio::test]
    async fn a_group_the_bot_is_muted_in_is_not() {
        let api = chat_api(chat_answer(SUPPORT_CHAT, "supergroup"), muted_answer()).await;

        let result = support_chat_usable(&api.bot, TelegramChatId::new(SUPPORT_CHAT), &me()).await;
        assert!(matches!(result, Err(Unusable::CannotPost)), "{result:?}");
    }

    #[tokio::test]
    async fn an_unknown_chat_disables_support() {
        let not_found = json!({ "ok": false, "error_code": 400, "description": "Bad Request: chat not found" });
        let api = chat_api(not_found, member_answer("member")).await;

        let result = support_chat_usable(&api.bot, TelegramChatId::new(SUPPORT_CHAT), &me()).await;
        assert!(matches!(result, Err(Unusable::Unknown(_))), "{result:?}");
    }

    #[tokio::test]
    async fn a_migrated_group_names_its_new_id() {
        let migrated = json!({
            "ok": false,
            "error_code": 400,
            "description": "Bad Request: group chat was upgraded to a supergroup chat",
            "parameters": { "migrate_to_chat_id": SUPPORT_CHAT }
        });
        let api = chat_api(migrated, member_answer("member")).await;

        let result = support_chat_usable(&api.bot, TelegramChatId::new(-4242), &me()).await;
        assert!(matches!(result, Err(Unusable::Migrated(ChatId(SUPPORT_CHAT)))), "{result:?}");
    }

    #[tokio::test]
    async fn a_private_chat_needs_no_membership() {
        let api = chat_api(chat_answer(USER_ID, "private"), member_answer("left")).await;
        let result = support_chat_usable(&api.bot, TelegramChatId::new(USER_ID), &me()).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(api.methods(), ["GetChat"]);
    }

    /// The only test that reports to the gauge, since the tests share it and run in parallel.
    #[tokio::test]
    async fn the_check_reports_its_outcome() {
        let usable = chat_api(chat_answer(SUPPORT_CHAT, "supergroup"), member_answer("member")).await;
        let chat = check(&usable, SUPPORT_CHAT).await;
        assert_eq!(chat, Some(TelegramChatId::new(SUPPORT_CHAT)));
        assert_verification(0);

        let unusable = chat_api(chat_answer(SUPPORT_CHAT, "supergroup"), muted_answer()).await;
        let chat = check(&unusable, SUPPORT_CHAT).await;
        assert_eq!(chat, None);
        assert_verification(1);
    }

    #[tokio::test]
    async fn an_unset_chat_asks_nothing() {
        let api = chat_api(chat_answer(SUPPORT_CHAT, "supergroup"), member_answer("member")).await;
        let chat = check_support_chat(&api.bot, None, &me()).await;
        assert_eq!(chat, None);
        assert!(api.methods().is_empty());
    }

    fn reply_message() -> Value {
        json!({
            "message_id": 20,
            "date": 0,
            "chat": { "id": SUPPORT_CHAT, "type": "supergroup", "title": "Support" },
            "from": { "id": 456, "is_bot": false, "first_name": "Helper" },
            "text": "Answer",
            "reply_to_message": {
                "message_id": TICKET_ID,
                "date": 0,
                "chat": { "id": SUPPORT_CHAT, "type": "supergroup", "title": "Support" },
                "from": { "id": BOT_ID, "is_bot": true, "first_name": "Bot" },
                "text": "🆘 Name\n123 · en\n\nQuestion",
                "entities": [
                    { "type": "text_link", "offset": 3, "length": 4, "url": "tg://user?id=123" },
                    { "type": "code", "offset": 8, "length": 3 }
                ]
            }
        })
    }

    fn answer_reply_message() -> Value {
        let private = json!({ "id": USER_ID, "type": "private", "first_name": "Name" });
        json!({
            "message_id": 31,
            "date": 0,
            "chat": private,
            "from": { "id": USER_ID, "is_bot": false, "first_name": "Name" },
            "text": "Thanks, but it still doesn't work",
            "reply_to_message": {
                "message_id": 30,
                "date": 0,
                "chat": private,
                "from": { "id": BOT_ID, "is_bot": true, "first_name": "Bot" },
                "text": "Answer",
                "reply_markup": {
                    "inline_keyboard": [[{ "text": "↩️ Reply", "callback_data": "support:10" }]]
                }
            }
        })
    }

    fn recipient(message: Value, support_chat: Option<i64>) -> Option<SupportRecipient> {
        let msg: Message = serde_json::from_value(message).expect("valid Telegram message");
        SupportService::new(support_chat.map(TelegramChatId::new))
            .extract_reply_recipient(&msg, TelegramUserId(BOT_ID))
    }

    fn recipient_chat(message: Value, support_chat: Option<i64>) -> Option<ChatId> {
        recipient(message, support_chat)
            .map(|recipient| recipient.chat_id)
    }

    fn answered_ticket(message: Value) -> Option<SupportTicket> {
        let msg: Message = serde_json::from_value(message).expect("valid Telegram message");
        let me: Me = serde_json::from_value(json!({
            "id": BOT_ID, "is_bot": true, "first_name": "Bot", "username": "bot",
            "can_join_groups": true, "can_read_all_group_messages": false,
            "supports_inline_queries": true, "can_connect_to_business_account": false,
            "has_main_web_app": false
        })).expect("valid Me");
        support_answered_ticket(msg, me)
    }

    /// A Bot API that answers `CopyMessage` with success or a refusal.
    async fn fake_api(copy_succeeds: bool) -> FakeApi {
        FakeApi::start(move |method| match method {
            "CopyMessage" if !copy_succeeds => json!({
                "ok": false,
                "error_code": 403,
                "description": "Forbidden: bot was blocked by the user"
            }),
            "CopyMessage" => json!({ "ok": true, "result": { "message_id": 30 } }),
            _ => json!({
                "ok": true,
                "result": {
                    "message_id": 21,
                    "date": 0,
                    "chat": { "id": SUPPORT_CHAT, "type": "supergroup", "title": "Support" },
                    "text": "Delivery failed"
                }
            }),
        }).await
    }

    async fn check(api: &FakeApi, chat_id: i64) -> Option<TelegramChatId> {
        check_support_chat(&api.bot, Some(TelegramChatId::new(chat_id)), &me()).await
    }

    fn assert_verification(expected: i64) {
        let series = format!("startup_verification_error{{kind=\"support_chat_unusable\"}} {expected}");
        let rendered = metrics::render_metrics();
        assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
    }

    async fn chat_api(get_chat: Value, get_chat_member: Value) -> FakeApi {
        FakeApi::start(move |method| match method {
            "GetChat" => get_chat.clone(),
            "GetChatMember" => get_chat_member.clone(),
            _ => json!({ "ok": false, "error_code": 400, "description": "Bad Request: unexpected" }),
        }).await
    }

    fn chat_answer(id: i64, kind: &str) -> Value {
        let mut chat = json!({
            "id": id,
            "type": kind,
            "max_reaction_count": 11,
            "accepted_gift_types": {
                "unlimited_gifts": false, "limited_gifts": false,
                "unique_gifts": false, "premium_subscription": false
            }
        });
        if kind == "private" {
            chat["first_name"] = json!("Name");
        } else {
            chat["title"] = json!("Support");
        }
        json!({ "ok": true, "result": chat })
    }

    fn member_answer(status: &str) -> Value {
        json!({
            "ok": true,
            "result": {
                "status": status,
                "user": { "id": BOT_ID, "is_bot": true, "first_name": "Bot" }
            }
        })
    }

    fn muted_answer() -> Value {
        json!({
            "ok": true,
            "result": {
                "status": "restricted",
                "user": { "id": BOT_ID, "is_bot": true, "first_name": "Bot" },
                "until_date": 0,
                "is_member": true,
                "can_send_messages": false,
                "can_send_audios": false,
                "can_send_documents": false,
                "can_send_photos": false,
                "can_send_videos": false,
                "can_send_video_notes": false,
                "can_send_voice_notes": false,
                "can_send_polls": false,
                "can_send_other_messages": false,
                "can_add_web_page_previews": false,
                "can_change_info": false,
                "can_invite_users": false,
                "can_pin_messages": false,
                "can_manage_topics": false
            }
        })
    }

    fn me() -> Me {
        serde_json::from_value(json!({
            "id": BOT_ID, "is_bot": true, "first_name": "Bot", "username": "TestBot",
            "can_join_groups": true, "can_read_all_group_messages": false,
            "supports_inline_queries": true, "can_connect_to_business_account": false,
            "has_main_web_app": false
        })).expect("couldn't build the bot's own user")
    }
}
