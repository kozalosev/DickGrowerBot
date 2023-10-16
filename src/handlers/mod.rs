mod dick;
mod help;
mod dod;
mod import;

use teloxide::Bot;
use teloxide::requests::Requester;
use teloxide::types::{Message, User};
use teloxide::types::ParseMode::Html;

pub use dick::*;
pub use help::*;
pub use dod::*;
pub use import::*;

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

pub fn ensure_lang_code(user: Option<&User>) -> String {
    user
        .map(|u| {
            u.language_code.clone()
                .or_else(|| {
                    log::warn!("no language_code for {}, using the default", u.id);
                    None
                })
        })
        .flatten()
        .unwrap_or("en".to_owned())
}

pub async fn reply_html(bot: Bot, msg: Message, answer: String) -> HandlerResult {
    // TODO: split to several messages if the answer is too long
    let mut answer = bot.send_message(msg.chat.id, answer);
    answer.parse_mode = Some(Html);
    answer.await?;
    Ok(())
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

        // TODO: delete before release (alongside with the ending of the error message)
        let allowed_chats = [-1001486665073, -1001631811756, -1001100294568, -1001947584857, -1001347968299];
        if !allowed_chats.contains(&msg.chat.id.0) {
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
        reply_html(bot, msg, answer).await
    }
}
