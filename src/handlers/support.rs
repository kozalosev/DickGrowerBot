//! A contact channel that doesn't expose an email address or a personal account: the bot relays
//! the message to the chat set in `SUPPORT_CHAT_ID`, and a reply to it there back to the user.
//!
//! It is the way out for a data deletion or access request — see the privacy policy — so it stays
//! above the ban gate in the dispatcher tree and must never write a row for its sender.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use autometrics::autometrics;
use rust_i18n::t;
use teloxide::Bot;
use crate::dialogue::CachedDialogueStorage;
use teloxide::macros::BotCommands;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::{Dialogue, Requester};
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{ChatId, Me, Message, MessageEntity, MessageEntityKind, ParseMode, User as TelegramUser};
use teloxide::utils::html;
use crate::domain::primitives::{LanguageCode, UserId};
use crate::domain::primitives::chat::TelegramChatId;
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
use crate::handlers::utils::get_full_name;
use crate::{metrics, reply_html};

/// One request per user per minute. A `const` rather than an environment variable: the knob is too
/// small to be worth a line in every deployment file.
const RATE_LIMIT: Duration = Duration::from_mins(1);

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
}

pub type SupportDialogue = Dialogue<SupportCommandState, CachedDialogueStorage<SupportCommandState>>;

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

#[derive(Clone)]
pub struct SupportService {
    chat_id: Option<TelegramChatId>,
    last_sent: Arc<Mutex<HashMap<UserId, Instant>>>,
}

impl SupportService {
    pub fn new(chat_id: Option<TelegramChatId>) -> Self {
        Self { chat_id, last_sent: Default::default() }
    }

    fn reply_recipient(&self, msg: &Message, bot_id: teloxide::types::UserId) -> Option<ChatId> {
        if self.chat_id != Some(msg.chat.id.into()) || msg.from.as_ref().is_some_and(|from| from.id == bot_id) {
            return None
        }
        let original = msg.reply_to_message()?;
        if original.from.as_ref()?.id != bot_id || original.chat.id != msg.chat.id {
            return None
        }
        support_user_id(original.text()?, original.entities()?)
    }

    async fn relay(
        &self,
        bot: &Bot,
        from: &TelegramUser,
        lang_code: &LanguageCode,
        text: &str,
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
            "🆘 <a href=\"tg://user?id={uid}\">{name}</a>\n<code>{uid}</code> · {lang_code}\n\n{text}",
            name = name.escaped(),
            text = html::escape(text),
        );
        bot.send_message(ChatId::from(chat_id), message)
            .parse_mode(ParseMode::Html)
            .disable_link_preview(true)
            .await?;
        Ok(Relayed::Sent)
    }

    fn pass_rate_limit(&self, uid: UserId) -> bool {
        let now = Instant::now();
        let mut last_sent = self.last_sent.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        last_sent.retain(|_, at| now.duration_since(*at) < RATE_LIMIT);
        last_sent.insert(uid, now).is_none()
    }
}

/// The user a reply in the support chat is meant for.
#[derive(Clone, Copy)]
pub struct SupportRecipient(ChatId);

pub fn support_reply_recipient(msg: Message, support: SupportService, me: Me) -> Option<SupportRecipient> {
    support.reply_recipient(&msg, me.user.id)
        .map(SupportRecipient)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg)))]
pub async fn support_reply_handler(bot: Bot, msg: Message, recipient: SupportRecipient) -> HandlerResult {
    let SupportRecipient(recipient) = recipient;
    if let Err(error) = bot.copy_message(recipient, msg.chat.id, msg.id).await {
        tracing::warn!(recipient = recipient.0, error = %error, "couldn't deliver support reply");
        bot.send_message(msg.chat.id, format!("Не удалось доставить ответ пользователю: {error}"))
            .reply_parameters(teloxide::types::ReplyParameters::new(msg.id))
            .await?;
    }
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
        relay(&bot, &msg, &support, &text, &lang_code).await?
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
            relay(&bot, &msg, &support, text, &lang_code).await?
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
) -> anyhow::Result<String> {
    let from = msg.from.as_ref().ok_or(anyhow::anyhow!("no from user"))?;
    let relayed = support.relay(bot, from, lang_code, text).await?;
    Ok(t!(relayed.tr_key(), locale = lang_code).to_string())
}

/// The link right after the emoji can only be the bot's own: the name and the text are escaped.
fn support_user_id(text: &str, entities: &[MessageEntity]) -> Option<ChatId> {
    if !text.starts_with("🆘 ") {
        return None
    }
    let first_mention = entities.iter().find(|entity| matches!(entity.kind,
        MessageEntityKind::TextLink { .. } | MessageEntityKind::TextMention { .. }))?;
    if first_mention.offset != 3 {
        return None
    }
    let uid = match &first_mention.kind {
        MessageEntityKind::TextLink { url } => url.as_str()
            .strip_prefix("tg://user?id=")?
            .parse::<u64>()
            .ok()?,
        MessageEntityKind::TextMention { user } => user.id.0,
        _ => return None,
    };
    let uid = i64::try_from(uid).ok()?;
    if uid <= 0 { return None }
    Some(ChatId(uid))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::sync::{Arc, Mutex};
    use axum::{Json, Router};
    use serde_json::{Value, json};

    const SUPPORT_CHAT: i64 = -1001234567890;
    const BOT_ID: u64 = 777;
    const USER_ID: i64 = 123;

    #[test]
    fn routes_text_and_photo_replies_to_the_user() {
        let message = reply_message();
        let to = recipient(message.clone(), Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));

        let mut mention = message.clone();
        mention["reply_to_message"]["entities"][0] = json!({
            "type": "text_mention", "offset": 3, "length": 4,
            "user": { "id": USER_ID, "is_bot": false, "first_name": "Name" }
        });
        let to = recipient(mention, Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));

        let mut photo = message;
        photo.as_object_mut().unwrap().remove("text");
        photo["photo"] = json!([{
            "file_id": "photo", "file_unique_id": "unique", "width": 1, "height": 1
        }]);
        let to = recipient(photo, Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));
    }

    #[test]
    fn ignores_unrelated_or_untrusted_replies() {
        let message = reply_message();
        let to = recipient(message.clone(), None);
        assert_eq!(to, None);
        let to = recipient(message.clone(), Some(-1009876));
        assert_eq!(to, None);

        let mut unrelated = message.clone();
        unrelated.as_object_mut().unwrap().remove("reply_to_message");
        let to = recipient(unrelated, Some(SUPPORT_CHAT));
        assert_eq!(to, None);

        let mut other_sender = message.clone();
        other_sender["reply_to_message"]["from"]["id"] = json!(999);
        let to = recipient(other_sender, Some(SUPPORT_CHAT));
        assert_eq!(to, None);

        let mut forged_name = message;
        forged_name["reply_to_message"]["text"] = json!("🆘 Name\n999 · en\n\nQuestion");
        let to = recipient(forged_name, Some(SUPPORT_CHAT));
        assert_eq!(to, Some(ChatId(USER_ID)));
    }

    #[tokio::test]
    async fn reports_a_copy_failure_in_the_support_chat() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let app = Router::new().fallback(move |request: axum::extract::Request| {
            let recorded = Arc::clone(&recorded);
            async move {
                let path = request.uri().path().to_owned();
                recorded.lock().unwrap().push(path.clone());
                if path.ends_with("/CopyMessage") {
                    Json(json!({
                        "ok": false,
                        "error_code": 403,
                        "description": "Forbidden: bot was blocked by the user"
                    }))
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
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let api_url = reqwest::Url::parse(&format!("http://{address}/")).unwrap();
        let bot = Bot::new("123:token").set_api_url(api_url);
        let msg: Message = serde_json::from_value(reply_message()).unwrap();

        support_reply_handler(bot, msg, SupportRecipient(ChatId(USER_ID)))
            .await.unwrap();

        let calls = calls.lock().unwrap().clone();
        assert_eq!(calls, ["/bot123:token/CopyMessage", "/bot123:token/SendMessage"]);
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
                "message_id": 10,
                "date": 0,
                "chat": { "id": SUPPORT_CHAT, "type": "supergroup", "title": "Support" },
                "from": { "id": BOT_ID, "is_bot": true, "first_name": "Bot" },
                "text": "🆘 Name\n123 · en\n\nQuestion",
                "entities": [
                    { "type": "text_link", "offset": 3, "length": 4, "url": "tg://user?id=123" },
                    { "type": "code", "offset": 9, "length": 3 }
                ]
            }
        })
    }

    fn recipient(message: Value, support_chat: Option<i64>) -> Option<ChatId> {
        let msg: Message = serde_json::from_value(message).expect("valid Telegram message");
        SupportService::new(support_chat.map(TelegramChatId::new))
            .reply_recipient(&msg, teloxide::types::UserId(BOT_ID))
    }
}
