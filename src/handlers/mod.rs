mod dick;
mod help;
mod start;
mod privacy;
mod dod;
mod import;
mod promo;
mod inline;
pub mod utils;
pub mod pvp;
pub mod perks;
pub mod loan;
pub mod stats;

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
pub use dod::*;
pub use import::*;
pub use inline::*;
pub use promo::*;
pub use loan::LoanCommands;
use crate::domain::LanguageCode;
use crate::handlers::utils::callbacks::CallbackDataWithPrefix;

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

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
                        log::error!("couldn't edit the message ({}:{}): {}", message.chat().id, message.id(), err);
                        Err(err)?;
                    }
                } else if let Some(inline_message_id) = callback_query.inline_message_id {
                    let mut edit_req = bot.edit_message_text_inline(&inline_message_id, text);
                    edit_req.parse_mode.replace(Html);
                    edit_req.reply_markup = keyboard;

                    let edit_req_resp = edit_req.await;
                    if let Err(err) = edit_req_resp {
                        log::error!("couldn't edit the message ({}): {}", inline_message_id, err);
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
    ($bot:ident, $msg:ident, $answer:expr) => {
        anyhow::Context::context(
            reply_html($bot, &$msg, $answer).await,
            format!("failed for {:?}", $msg)
        )?
    };
}

pub async fn send_error_callback_answer(bot: Bot, query: CallbackQuery, tr_key: &str) -> HandlerResult {
    let lang_code = LanguageCode::from_user(&query.from);
    bot.answer_callback_query(query.id)
        .show_alert(true)
        .text(t!(tr_key, locale = &lang_code))
        .await?;
    Ok(())
}

pub mod checks {
    use rust_i18n::t;
    use teloxide::Bot;
    use teloxide::types::{ChatId, Message};
    use crate::domain::LanguageCode;
    use super::{HandlerResult, reply_html};

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
        handle_by_reply(bot, msg, "errors.not_group_chat").await
    }
    
    /// Part of the PeezyBigDBot fork
    pub fn is_not_allowed_chat(chat_id: ChatId) -> impl Fn(Message) -> bool {
        move |msg| !msg.chat.is_private() && msg.chat.id != chat_id
    }
    
    pub async fn handle_not_allowed_chat(bot: Bot, msg: Message) -> HandlerResult {
        handle_by_reply(bot, msg, "errors.private_bot").await
    }
    // end of the fork code

    async fn handle_by_reply(bot: Bot, msg: Message, answer_key: &str) -> HandlerResult {
        let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
        let answer = t!(answer_key, locale = &lang_code);
        reply_html(bot, &msg, answer).await?;
        Ok(())
    }

    pub mod inline {
        use futures::TryFutureExt;
        use teloxide::Bot;
        use teloxide::payloads::AnswerInlineQuerySetters;
        use teloxide::prelude::{ChatId, InlineQuery, Requester};
        use teloxide::types::{ChatType, ChosenInlineResult};
        use crate::config::AppConfig;
        use crate::handlers::try_resolve_chat_id;
        use crate::repo::Repositories;
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


        /// Part of the PeezyBigDBot fork
        pub async fn is_not_allowed_chat(repos: Repositories, cfg: AppConfig, chosen_result: ChosenInlineResult) -> bool {
            let maybe_chat_in_sync = chosen_result.inline_message_id.as_ref()
                .and_then(try_resolve_chat_id)
                .map(|chat_id| repos.chats.get_chat(chat_id.into()));
            if let Some(chat_in_sync_future) = maybe_chat_in_sync {
                chat_in_sync_future
                    .map_ok(|res| res
                        .filter(|c| c.chat_id.is_some() && c.chat_instance.is_some())
                        .and_then(|c| c.chat_id)
                        .map(ChatId))
                    .await
                    .inspect_err(|err| log::error!("[checks:inline:is_not_allowed_chat] error: {}", err))
                    .ok()
                    .flatten()
                    .map(|chat_id| chat_id != cfg.peezy_fork_settings.allowed_chat_id)
                    .unwrap_or(true)
            } else {
                true
            }
        }

        pub async fn handle_no_op() -> HandlerResult {
            Ok(())
        }
        // end of the fork code
    }

    /// Part of the PeezyBigDBot fork
    pub mod callback {
        use futures::TryFutureExt;
        use rust_i18n::t;
        use teloxide::Bot;
        use teloxide::payloads::AnswerCallbackQuerySetters;
        use teloxide::prelude::ChatId;
        use teloxide::requests::Requester;
        use teloxide::types::CallbackQuery;
        use crate::config::AppConfig;
        use crate::domain::LanguageCode;
        use crate::handlers::{try_resolve_chat_id, HandlerResult};
        use crate::repo::{ChatIdKind, Repositories};

        pub async fn is_not_allowed_chat(repos: Repositories, cfg: AppConfig, query: CallbackQuery) -> bool {
            let chat_id_kind = query.inline_message_id.as_ref()
                .and_then(try_resolve_chat_id)
                .map(ChatIdKind::from)
                .unwrap_or(ChatIdKind::from(query.chat_instance));
            repos.chats.get_chat(chat_id_kind)
                .map_ok(|res| res
                    .filter(|c| c.chat_id.is_some() && c.chat_instance.is_some())
                    .and_then(|c| c.chat_id)
                    .map(ChatId))
                .await
                .inspect_err(|err| log::error!("[checks:callback:is_not_allowed_chat] error: {}", err))
                .ok()
                .flatten()
                .map(|chat_id| chat_id != cfg.peezy_fork_settings.allowed_chat_id)
                .unwrap_or(true)
        }

        pub async fn handle_not_allowed_chat(bot: Bot, query: CallbackQuery) -> HandlerResult {
            let lang_code = LanguageCode::from_user(&query.from);
            let answer = t!("errors.private_bot", locale = &lang_code);
            bot.answer_callback_query(&query.id)
                .show_alert(true)
                .text(answer)
                .await
                .map(|_| ())
                .map_err(Into::into)
        }
    }

}
