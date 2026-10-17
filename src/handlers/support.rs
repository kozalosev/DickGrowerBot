//! A contact channel that doesn't expose an email address or a personal account: the bot relays
//! the message to the chat set in `SUPPORT_CHAT_ID`, a reply to it there back to the user, and the
//! user's answer to that reply into the same thread.
//!
//! It is the way out for a data deletion or access request — see the privacy policy — so it stays
//! above the ban gate in the dispatcher tree and must never write a row for its sender.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use autometrics::autometrics;
use derive_more::Display;
use rust_i18n::t;
use teloxide::Bot;
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
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::handlers::utils::get_full_name;
use crate::{metrics, reply_html};

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
    last_sent: Arc<Mutex<HashMap<UserId, Instant>>>,
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

enum Relayed {
    Sent,
    TooOften,
    Disabled,
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
        t!("commands.support.request", locale = &lang_code).to_string()
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
            t!("commands.support.cancelled", locale = &lang_code).to_string()
        }
        Some(text) => {
            dialogue.exit().await?;
            relay(&bot, &msg, &support, text, &lang_code, ticket).await?
        }
        None => t!("commands.support.request", locale = &lang_code).to_string()
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
) -> anyhow::Result<String> {
    let from = msg.from.as_ref().ok_or(anyhow::anyhow!("no from user"))?;
    let relayed = support.relay(bot, from, lang_code, text, ticket).await?;
    Ok(t!(relayed.tr_key(), locale = lang_code).to_string())
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
    use std::sync::{Arc, Mutex};
    use axum::{Json, Router};
    use serde_json::{Value, json};

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
        let (calls, server, bot) = fake_api(true).await;
        let msg: Message = serde_json::from_value(reply_message()).unwrap();
        let recipient = recipient(reply_message(), Some(SUPPORT_CHAT))
            .expect("a recipient");

        support_reply_handler(bot, msg, recipient)
            .await.unwrap();

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let (path, body) = &calls[0];
        assert_eq!(path, "/bot123:token/CopyMessage");
        let data = &body["reply_markup"]["inline_keyboard"][0][0]["callback_data"];
        assert_eq!(data, "support:10");
        server.abort();
    }

    #[tokio::test]
    async fn reports_a_copy_failure_in_the_support_chat() {
        let (calls, server, bot) = fake_api(false).await;
        let msg: Message = serde_json::from_value(reply_message()).unwrap();
        let recipient = recipient(reply_message(), Some(SUPPORT_CHAT))
            .expect("a recipient");

        support_reply_handler(bot, msg, recipient)
            .await.unwrap();

        let paths: Vec<_> = calls.lock().unwrap()
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(paths, ["/bot123:token/CopyMessage", "/bot123:token/SendMessage"]);
        server.abort();
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

    type Calls = Arc<Mutex<Vec<(String, Value)>>>;

    /// A Bot API that records each call and answers `CopyMessage` with success or a refusal.
    async fn fake_api(copy_succeeds: bool) -> (Calls, tokio::task::JoinHandle<()>, Bot) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let app = Router::new().fallback(move |request: axum::extract::Request| {
            let recorded = Arc::clone(&recorded);
            async move {
                let path = request.uri().path().to_owned();
                let body = axum::body::to_bytes(request.into_body(), usize::MAX)
                    .await.unwrap();
                let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
                recorded.lock().unwrap().push((path.clone(), body));
                if path.ends_with("/CopyMessage") && !copy_succeeds {
                    Json(json!({
                        "ok": false,
                        "error_code": 403,
                        "description": "Forbidden: bot was blocked by the user"
                    }))
                } else if path.ends_with("/CopyMessage") {
                    Json(json!({ "ok": true, "result": { "message_id": 30 } }))
                } else {
                    Json(json!({
                        "ok": true,
                        "result": {
                            "message_id": 21,
                            "date": 0,
                            "chat": { "id": SUPPORT_CHAT, "type": "supergroup", "title": "Support" },
                            "text": "Delivery failed"
                        }
                    }))
                }
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let api_url = reqwest::Url::parse(&format!("http://{address}/")).unwrap();
        let bot = Bot::new("123:token").set_api_url(api_url);
        (calls, server, bot)
    }
}
