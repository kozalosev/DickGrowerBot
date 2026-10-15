use anyhow::anyhow;
use autometrics::autometrics;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::AnswerCallbackQuerySetters;
use teloxide::prelude::{CallbackQuery, Message, Requester};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use crate::config::MessageGroup::Notice;
use crate::domain::primitives::{LanguageCode, UserId};
use crate::domain::primitives::chat::ChatIdKind;
use crate::handlers::{reply_html, HandlerDeps, HandlerResult};
use crate::handlers::utils::{callbacks, get_full_name};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::{metrics, reply_html, reply_html_ephemeral};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum MentionsCommands {
    #[command(description = "mentions")]
    Mentions,
}

/// In a private chat, shows whether the user is mentioned in the shrink summaries of the chats they
/// made no choice for, with a button to change it. A group gets a pointer to the button under the
/// summary instead, which is where the choice for one chat is made.
#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn mentions_cmd_handler(bot: Bot, msg: Message, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { repos, self_destruction, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_MENTIONS.invoked();

    if !msg.chat.is_private() {
        reply_html_ephemeral!(bot, msg, t!("commands.mentions.group_hint", locale = &lang_code),
            self_destruction, Notice, lang_code);
        return Ok(())
    }

    let from = msg.from.as_ref()
        .ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let uid = UserId::from(from);
    repos.users.create_or_update(uid, &get_full_name(from)).await?;
    let mentions = repos.users.get_shrink_mentions(uid).await?;
    reply_html!(bot, msg, render_state(mentions, &lang_code),
        reply_markup = ReplyMarkup::InlineKeyboard(build_keyboard(mentions, &lang_code)));
    Ok(())
}

pub fn callback_filter(query: CallbackQuery) -> bool {
    MentionsCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn mentions_callback_handler(bot: Bot, query: CallbackQuery, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let uid = UserId::from(&query.from);

    match MentionsCallbackData::parse(&query)? {
        MentionsCallbackData::Set(mentions) => {
            repos.users.set_shrink_mentions(uid, mentions).await?;
            metrics::CMD_MENTIONS.finished();
            let params = callbacks::get_params_for_message_edit(&query)?;
            callbacks::answer_and_edit_page(&bot, &query, &params, render_state(mentions, &lang_code),
                build_keyboard(mentions, &lang_code)).await?;
        },
        MentionsCallbackData::ToggleHere => {
            let message = query.message.as_ref()
                .ok_or(anyhow!("a mentions callback without an attached message"))?;
            let chat_id = ChatIdKind::from(message.chat().id);
            let key = match repos.player_settings.toggle_shrink_mentions(&chat_id, uid).await? {
                Some(true) => "commands.shrink.mentions.resumed",
                Some(false) => "commands.shrink.mentions.stopped",
                None => "commands.shrink.mentions.not_playing",
            };
            metrics::CMD_MENTIONS.finished();
            bot.answer_callback_query(query.id)
                .show_alert(true)
                .text(t!(key, locale = &lang_code))
                .await?;
        },
    }
    Ok(())
}

fn render_state(mentions: bool, lang_code: &LanguageCode) -> String {
    let key = if mentions { "commands.mentions.enabled" } else { "commands.mentions.disabled" };
    t!(key, locale = lang_code).to_string()
}

fn build_keyboard(mentions: bool, lang_code: &LanguageCode) -> InlineKeyboardMarkup {
    let key = if mentions { "commands.mentions.buttons.disable" } else { "commands.mentions.buttons.enable" };
    InlineKeyboardMarkup::new([[InlineKeyboardButton::callback(
        t!(key, locale = lang_code),
        MentionsCallbackData::Set(!mentions).to_data_string())]])
}

/// Callback payload of the mention switches. The wire format is `mentions:on`/`mentions:off` for the
/// choice made in a private chat, and `mentions:here` for the button under a shrink summary, which
/// flips the choice of whoever pressed it for that chat alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, derive_more::Display)]
pub(crate) enum MentionsCallbackData {
    #[display("{}", if *_0 { "on" } else { "off" })]
    Set(bool),
    #[display("here")]
    ToggleHere,
}

impl CallbackDataWithPrefix for MentionsCallbackData {
    fn prefix() -> &'static str {
        "mentions"
    }
}

impl TryFrom<String> for MentionsCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        match data.as_str() {
            "on" => Ok(Self::Set(true)),
            "off" => Ok(Self::Set(false)),
            "here" => Ok(Self::ToggleHere),
            _ => Err(InvalidCallbackDataBuilder(&data).split_err()),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::handlers::utils::callbacks::{build_callback_query, CallbackDataWithPrefix};
    use super::MentionsCallbackData;

    #[test]
    fn test_callback_data_wire_format() {
        assert_eq!(MentionsCallbackData::Set(true).to_data_string(), "mentions:on");
        assert_eq!(MentionsCallbackData::Set(false).to_data_string(), "mentions:off");
        assert_eq!(MentionsCallbackData::ToggleHere.to_data_string(), "mentions:here");
    }

    #[test]
    fn test_callback_data_round_trip() {
        for data in [MentionsCallbackData::Set(true), MentionsCallbackData::Set(false), MentionsCallbackData::ToggleHere] {
            let query = build_callback_query(data.to_data_string());
            let parsed = MentionsCallbackData::parse(&query)
                .expect("couldn't parse the callback data");
            assert_eq!(parsed, data);
        }
    }
}
