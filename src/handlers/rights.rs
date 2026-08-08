//! Keeping track of what the bot is allowed to do in a chat.
//!
//! Telegram sends a `my_chat_member` update whenever the bot's **own** status somewhere changes —
//! when it is added, promoted or demoted — and that update is in the default set, so it arrives
//! without asking. It is the cheapest and the most accurate source there is, which is why nothing
//! here polls: the bot is told.
//!
//! The one right that matters is deleting other members' messages, which the self-destruction of
//! a command depends on (see [`crate::handlers::utils::SelfDestructionService`]).

use autometrics::autometrics;
use teloxide::types::ChatMemberUpdated;
use crate::cache::Cache;
use crate::handlers::HandlerResult;

/// Fires when the bot's own right to delete messages has just changed. Private chats are skipped:
/// nothing is ever cleaned up there.
pub fn bot_rights_changed_filter(upd: ChatMemberUpdated) -> bool {
    !upd.chat.is_private()
        && upd.old_chat_member.can_delete_messages() != upd.new_chat_member.can_delete_messages()
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = upd.chat.id.0))]
pub async fn bot_rights_changed_handler(upd: ChatMemberUpdated, cache: Cache) -> HandlerResult {
    let may_delete = upd.new_chat_member.can_delete_messages();
    tracing::info!(may_delete, "the bot's rights in the chat have changed");
    cache.set_bot_admin(upd.chat.id, may_delete).await;
    Ok(())
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
    fn a_promotion_that_grants_the_right_is_caught() {
        let upd = updated(supergroup(), member(), admin(true));
        assert!(bot_rights_changed_filter(upd));
    }

    #[test]
    fn a_demotion_that_takes_it_away_is_caught() {
        let upd = updated(supergroup(), admin(true), member());
        assert!(bot_rights_changed_filter(upd));
    }

    #[test]
    fn a_promotion_without_that_right_changes_nothing() {
        // An administrator who may not delete messages is, for our purposes, an ordinary member.
        let upd = updated(supergroup(), member(), admin(false));
        assert!(!bot_rights_changed_filter(upd));
    }

    #[test]
    fn a_private_chat_is_never_of_interest() {
        let upd = updated(private(), member(), admin(true));
        assert!(!bot_rights_changed_filter(upd));
    }

}
