mod dick;
mod help;
mod dod;
mod import;
mod promo;
mod inline;
mod utils;
pub mod pvp;

use std::borrow::ToOwned;
use teloxide::Bot;
use teloxide::payloads::SendMessage;
use teloxide::requests::{JsonRequest, Requester};
use teloxide::types::{Message, User};
use teloxide::types::ParseMode::Html;

pub use dick::*;
pub use help::*;
pub use dod::*;
pub use import::*;
pub use inline::*;
pub use promo::*;

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

pub fn ensure_lang_code(user: Option<&User>) -> String {
    user
        .and_then(|u| {
            u.language_code.as_ref()
                .or_else(|| {
                    log::warn!("no language_code for {}, using the default", u.id);
                    None
                })
        })
        .map(|code| match &code[..2] {
            "uk" | "be" => "ru",
            _ => code
        })
        .unwrap_or("en")
        .to_owned()
}

pub fn reply_html(bot: Bot, msg: Message, answer: String) -> JsonRequest<SendMessage> {
    // TODO: split to several messages if the answer is too long
    let mut answer = bot.send_message(msg.chat.id, answer);
    answer.parse_mode = Some(Html);
    if msg.chat.is_group() || msg.chat.is_supergroup() {
        answer.reply_to_message_id.replace(msg.id);
    }
    answer
}

pub mod checks {
    use rust_i18n::t;
    use teloxide::Bot;
    use teloxide::types::Message;
    use super::{ensure_lang_code, HandlerResult, reply_html};

    pub fn is_group_chat(msg: Message) -> bool {
        if msg.chat.is_private() || msg.chat.is_channel() {
            return false
        }
        true
    }

    pub fn is_not_group_chat(msg: Message) -> bool {
        !is_group_chat(msg)
    }

    pub async fn handle_not_group_chat(bot: Bot, msg: Message) -> HandlerResult {
        let lang_code = ensure_lang_code(msg.from());
        let answer = t!("errors.not_group_chat", locale = &lang_code);
        reply_html(bot, msg, answer).await?;
        Ok(())
    }

    pub mod inline {
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

        pub async fn handle_not_group_chat(bot: Bot, query: InlineQuery) -> HandlerResult {
            bot.answer_inline_query(query.id, vec![])
                .is_personal(true)
                .cache_time(1)
                .await?;
            Ok(())
        }
    }
}
