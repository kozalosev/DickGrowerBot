//! Onboarding of legacy (basic) groups.
//!
//! Telegram's `inline_message_id` encodes the chat only for supergroups and channels. In a
//! legacy group it decodes to something unrelated (typically the inline sender's user id), so
//! the real `chat_id` can never be recovered from an inline invocation — a hard API limitation.
//!
//! The only update carrying **both** the real `chat_id` and the `chat_instance` is a *non-inline*
//! callback: a tap on a button of a regular message the bot posted into the group. This module
//! posts such a message and, on the tap, stores both ids in a single `Chats` row — *anchoring*
//! the group. Afterward inline usage (keyed by `chat_instance`) merges into that same row.
//!
//! Until a legacy group is anchored, the chat-stateful **commands** are rejected (see
//! [`crate::handlers::checks::require_anchored_group`]). Inline invocations are not, and can't
//! be: an inline update carries no chat id to run the check against. Nor do they need to be —
//! they key off `chat_instance` throughout, which is self-consistent on its own.
//!
//! What the gate buys is that a chat can't accumulate two separate halves of its state — command
//! play on `chat_id`, inline play on `chat_instance` — before anyone has had the chance to pair
//! the two ids up.

use anyhow::anyhow;
use autometrics::autometrics;
use derive_more::Display;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::{CallbackQuery, ChatId, Message, Requester};
use teloxide::types::{ChatMemberUpdated, ChatMigration, InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

use crate::config::AppConfig;
use crate::domain::primitives::LanguageCode;
use crate::domain::primitives::chat::{ChatIdFull, ChatIdSource, TelegramChatId, TelegramChatInstanceId};
use crate::handlers::{HandlerDeps, HandlerResult};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::metrics;
use crate::repo::ChatMigrationOutcome;

#[derive(Display)]
#[display("activate")]
pub(crate) struct SetupCallbackData;

impl CallbackDataWithPrefix for SetupCallbackData {
    fn prefix() -> &'static str {
        "setup"
    }
}

impl TryFrom<String> for SetupCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        match data.as_str() {
            "activate" => Ok(Self),
            _ => Err(InvalidCallbackDataBuilder(&data).split_err())
        }
    }
}

/// Posts the message whose button anchors the group once tapped.
///
/// Deliberately not a reply to any particular message and not restricted to admins: the
/// `chat_instance` is chat-level, so a tap by *any* member yields the same pairing.
pub async fn send_setup_message(bot: &Bot, chat_id: ChatId, lang_code: &LanguageCode) -> anyhow::Result<()> {
    let btn_label = t!("setup.button", locale = lang_code);
    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback(btn_label, SetupCallbackData.to_data_string())
    ]]);
    bot.send_message(chat_id, t!("setup.text", locale = lang_code))
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await?;
    Ok(())
}

/// Fires when the bot has just been added to a legacy group: the earliest moment we can ask for
/// the anchoring tap, so the members aren't interrupted mid-command later.
pub fn added_to_legacy_group_filter(upd: ChatMemberUpdated, config: AppConfig) -> bool {
    config.features.chats_merging
        && upd.chat.is_group()
        && upd.new_chat_member.is_present()
        && !upd.old_chat_member.is_present()
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = upd.chat.id.0, uid = upd.from.id.0))]
pub async fn added_to_legacy_group_handler(bot: Bot, upd: ChatMemberUpdated) -> HandlerResult {
    let lang_code = LanguageCode::from_user(&upd.from);
    send_setup_message(&bot, upd.chat.id, &lang_code).await?;
    Ok(())
}

/// Telegram reports a group→supergroup migration twice: once in the old group
/// ([`ChatMigration::To`]) and once in the new supergroup ([`ChatMigration::From`]). Either is
/// enough to repoint the chat; the other then finds nothing left to do.
#[inline]
pub fn migration_filter(msg: Message) -> bool {
    msg.chat_migration().is_some()
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, lang_code = %lang_code))]
pub async fn migration_handler(
    bot: Bot,
    msg: Message,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    // `From` is the announcement that lands in the new supergroup, which is where the members
    // now are — the old group is left behind, so only this side is worth answering, and picking
    // one side also keeps the two announcements from producing two messages.
    let (old, new, in_new_chat) = match msg.chat_migration() {
        Some(ChatMigration::To { chat_id: new }) => (msg.chat.id, *new, false),
        Some(ChatMigration::From { chat_id: old }) => (*old, msg.chat.id, true),
        None => return Err(anyhow!("the migration handler got a message without a migration").into())
    };
    let outcome = repos.chats
        .migrate_chat_id(&TelegramChatId::from(old), &TelegramChatId::from(new))
        .await?;

    // Both announcements reach the same verdict, so everything below happens once, on the side
    // that lands where the members now are.
    if !in_new_chat {
        return Ok(())
    }
    metrics::CHAT_MIGRATION.record(outcome);

    // An instance is reissued along with the chat id, so a row keyed by one is unreachable after a
    // migration. Both outcomes below leave such a row behind; they differ in whether the rest of
    // the chat came across, which is too big a difference to paper over with one message. Neither
    // can tell "the inline half is gone" from "nobody ever played here", hence the careful wording.
    let lost_text_key = match outcome {
        ChatMigrationOutcome::Untraceable => {
            log::warn!("the chat migrated from {old} to {new} was unknown by its old id; \
                anything it had under a chat_instance is now unreachable");
            Some("migration.lost")
        }
        ChatMigrationOutcome::MigratedUnanchored => {
            log::warn!("the chat migrated from {old} to {new} had never been anchored; \
                anything it played through inline mode is now unreachable");
            Some("migration.lost_inline")
        }
        _ => None
    };
    if let Some(key) = lost_text_key {
        bot.send_message(msg.chat.id, t!(key, locale = &lang_code)).await?;
    }
    Ok(())
}

#[inline]
pub fn callback_filter(query: CallbackQuery) -> bool {
    SetupCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0))]
pub async fn callback_handler(bot: Bot, query: CallbackQuery, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { repos, .. } = deps;
    SetupCallbackData::parse(&query)?;
    let lang_code = LanguageCode::from_user(&query.from);

    // A non-inline callback is the whole point here: `message` gives us the real chat id, which
    // `chat_instance` alone never could. An inline one carries no chat id, so it can't anchor.
    let message = query.message.as_ref()
        .ok_or(anyhow!("the setup callback must be invoked from a non-inline message"))?;
    let chat_id = ChatIdFull {
        id: TelegramChatId::from(message.chat().id),
        instance: TelegramChatInstanceId::new(query.chat_instance.clone()),
    };
    log::info!("anchoring the chat: {chat_id}");
    repos.chats.upsert_chat(&chat_id.to_partiality(ChatIdSource::default())).await?;

    bot.edit_message_text(message.chat().id, message.id(),
            t!("setup.activated", locale = &lang_code))
        .await?;
    bot.answer_callback_query(query.id).await?;
    Ok(())
}
