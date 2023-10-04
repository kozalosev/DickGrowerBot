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
    let mut answer = bot.send_message(msg.chat.id, answer);
    answer.parse_mode = Some(Html);
    answer.await?;
    Ok(())
}
