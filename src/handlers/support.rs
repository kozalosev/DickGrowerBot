//! A contact channel that doesn't expose an email address or a personal account: the bot relays
//! the message to the chat set in `SUPPORT_CHAT_ID`.
//!
//! It is the way out for a data deletion or access request — see the privacy policy — so it stays
//! above the ban gate in the dispatcher tree and must never write a row for its sender.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use autometrics::autometrics;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::macros::BotCommands;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::{Dialogue, Requester};
use teloxide::sugar::request::RequestLinkPreviewExt;
use teloxide::types::{ChatId, Message, ParseMode, User as TelegramUser};
use teloxide::utils::html;
use crate::domain::primitives::{LanguageCode, UserId};
use crate::domain::primitives::chat::TelegramChatId;
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
use crate::handlers::utils::get_full_name;
use crate::{metrics, reply_html};

/// One request per user per minute. A `const` rather than an environment variable: the knob is too
/// small to be worth a line in every deployment file.
const RATE_LIMIT: Duration = Duration::from_secs(60);

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum SupportCommands {
    #[command(description = "support")]
    Support(String),
}

#[derive(Clone, Default)]
pub enum SupportCommandState {
    #[default]
    Start,
    Requested,
}

pub type SupportDialogue = Dialogue<SupportCommandState, InMemStorage<SupportCommandState>>;

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
            Relayed::Disabled => "commands.support.disabled",
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
