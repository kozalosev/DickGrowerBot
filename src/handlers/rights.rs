//! Keeping track of what the bot is allowed to do in a chat.
//!
//! Telegram sends a `my_chat_member` update whenever the bot's **own** status somewhere changes —
//! when it is added, promoted or demoted — and that update is in the default set, so it arrives
//! without asking. It is the cheapest and the most accurate source there is, which is why nothing
//! here polls: the bot is told.
//!
//! The one right tracked here is deleting other members' messages.

use autometrics::autometrics;
use teloxide::types::{ChatId, ChatMemberUpdated};
use std::time::Duration;
use crate::cache::{Cache, CacheKey};
use crate::config::AppConfig;
use crate::domain::primitives::chat::TelegramChatId;
use crate::metrics;

/// What the cache last knew about the bot's right to delete messages here, or `None` when nothing
/// is known.
pub async fn cached_deletion_right(cache: &Cache, chat_id: ChatId) -> Option<bool> {
    let known = cache.get_flag(BotAdminKey::new(chat_id)).await;
    match known {
        Some(_) => metrics::BOT_ADMIN_LOOKUP.cache_hit(),
        None => metrics::BOT_ADMIN_LOOKUP.miss(),
    }
    known
}

/// Remembers what Telegram said, or showed, about that right, for as long as `ttl`.
pub async fn remember_deletion_right(cache: &Cache, chat_id: ChatId, may_delete: bool, ttl: Duration) {
    cache.set_flag(BotAdminKey::new(chat_id), may_delete, ttl).await
}

/// Records the bot's rights on every change of its own status.
///
/// Not an endpoint. The same update may be the one that adds the bot to a legacy group, and that
/// branch answers with the setup message and consumes it — including when the bot is added as an
/// administrator in one step, which is a single update with no second one to follow. Recorded here
/// instead, before the branches, so nothing depends on which of them wins.
///
/// The value is written on every update rather than only when it differs from the previous one.
/// The question is not whether the right *changed* but whether it is now *known*, and the two come
/// apart exactly where it matters: a bot added to a chat as an ordinary member goes from `left` to
/// `member`, neither of which may delete. Nothing changed, and yet this is the moment the bot is
/// told what it may do there — and the moment it starts answering commands.
///
/// Only a change is worth a log line. `my_chat_member` arrives when the bot is added, promoted,
/// demoted or removed, so the write is rare whatever the bot's traffic.
#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = upd.chat.id.0))]
pub async fn remember_bot_rights(upd: ChatMemberUpdated, cache: Cache, config: AppConfig) {
    let Some(may_delete) = right_to_remember(&upd) else {
        return
    };
    if may_delete != upd.old_chat_member.can_delete_messages() {
        tracing::info!(may_delete, "the bot's rights in the chat have changed");
    }
    remember_deletion_right(&cache, upd.chat.id, may_delete, config.caches.bot_admin).await;
}

/// What to remember for this update, or `None` for a chat nothing is ever cleaned up in.
fn right_to_remember(upd: &ChatMemberUpdated) -> Option<bool> {
    (!upd.chat.is_private()).then(|| upd.new_chat_member.can_delete_messages())
}

/// Keyed by chat, because the right is granted per chat.
#[derive(derive_more::Display)]
#[display("chat:{_0}:bot_admin")]
struct BotAdminKey(TelegramChatId);

impl CacheKey for BotAdminKey {}

impl BotAdminKey {
    fn new(chat_id: ChatId) -> Self {
        Self(TelegramChatId::from(chat_id))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    /// Builds the update out of the JSON Telegram itself would have sent: `ChatMemberUpdated` has
    /// far more fields than a test wants to name, and the member variants are what is being tested.
    fn updated(chat: serde_json::Value, old: serde_json::Value, new: serde_json::Value) -> ChatMemberUpdated {
        serde_json::from_value(serde_json::json!({
            "chat": chat,
            "from": { "id": 1, "is_bot": false, "first_name": "Tester" },
            "date": 0,
            "old_chat_member": old,
            "new_chat_member": new,
        })).expect("couldn't build the update")
    }

    fn supergroup() -> serde_json::Value {
        serde_json::json!({ "id": -1001234567890i64, "type": "supergroup", "title": "Test" })
    }

    fn private() -> serde_json::Value {
        serde_json::json!({ "id": 1, "type": "private", "first_name": "Tester" })
    }

    fn bot() -> serde_json::Value {
        serde_json::json!({ "id": 42, "is_bot": true, "first_name": "Bot", "username": "test_bot" })
    }

    fn member() -> serde_json::Value {
        serde_json::json!({ "status": "member", "user": bot() })
    }

    fn left() -> serde_json::Value {
        serde_json::json!({ "status": "left", "user": bot() })
    }

    /// An administrator, with every right Telegram sends spelled out — `can_delete_messages` is
    /// the one being varied.
    fn admin(can_delete: bool) -> serde_json::Value {
        serde_json::json!({
            "status": "administrator",
            "user": bot(),
            "can_be_edited": false,
            "is_anonymous": false,
            "can_manage_chat": true,
            "can_delete_messages": can_delete,
            "can_manage_video_chats": false,
            "can_restrict_members": false,
            "can_promote_members": false,
            "can_change_info": false,
            "can_invite_users": false,
            "can_post_stories": false,
            "can_edit_stories": false,
            "can_delete_stories": false,
        })
    }

    #[test]
    fn a_promotion_that_grants_the_right_is_remembered() {
        let upd = updated(supergroup(), member(), admin(true));
        assert_eq!(right_to_remember(&upd), Some(true));
    }

    #[test]
    fn a_demotion_that_takes_it_away_is_remembered() {
        let upd = updated(supergroup(), admin(true), member());
        assert_eq!(right_to_remember(&upd), Some(false));
    }

    #[test]
    fn an_administrator_who_may_not_delete_counts_as_an_ordinary_member() {
        let upd = updated(supergroup(), member(), admin(false));
        assert_eq!(right_to_remember(&upd), Some(false));
    }

    /// The one that adds the bot to a legacy group as an administrator: the setup branch consumes
    /// this update, so it has to be recorded before the branches rather than by one of them.
    #[test]
    fn joining_a_group_as_an_administrator_is_remembered() {
        let upd = updated(supergroup(), left(), admin(true));
        assert_eq!(right_to_remember(&upd), Some(true));
    }

    /// Neither status may delete, so nothing *changed* — but this is the update that says the bot
    /// is an ordinary member here, and it arrives before the first command does.
    #[test]
    fn joining_a_group_as_an_ordinary_member_is_remembered_too() {
        let upd = updated(supergroup(), left(), member());
        assert_eq!(right_to_remember(&upd), Some(false));
    }

    #[test]
    fn a_private_chat_is_never_of_interest() {
        let upd = updated(private(), member(), admin(true));
        assert_eq!(right_to_remember(&upd), None);
    }

}
