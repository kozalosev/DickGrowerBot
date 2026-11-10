//! Keeping the titles of the groups the bot plays in.
//!
//! A title is taken from our own commands and from the service message of a rename, so a group
//! where the bot reads every message costs nothing for the messages it ignores.

use teloxide::types::{Chat, ChatMemberUpdated, Me, Message};
use crate::domain::primitives::chat::{ChatTitle, TelegramChatId};
use crate::handlers::checks::is_our_command;
use crate::repo::Repositories;

/// The chat and its title, when the message is worth taking the title from.
pub fn title_to_remember(msg: Message, me: Me) -> Option<(TelegramChatId, ChatTitle)> {
    (msg.new_chat_title().is_some() || is_our_command(&msg, &me))
        .then(|| group_title(&msg.chat))
        .flatten()
}

pub async fn remember_chat_title(repos: Repositories, (chat_id, title): (TelegramChatId, ChatTitle)) {
    store(&repos, chat_id, title).await
}

pub async fn remember_member_chat_title(repos: Repositories, upd: ChatMemberUpdated) {
    if let Some((chat_id, title)) = group_title(&upd.chat) {
        store(&repos, chat_id, title).await
    }
}

#[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
async fn store(repos: &Repositories, chat_id: TelegramChatId, title: ChatTitle) {
    match repos.chats.set_title(&chat_id, &title).await {
        Ok(true) => tracing::debug!("the chat's title changed"),
        Ok(false) => {},
        Err(e) => tracing::warn!(error = format!("{e:#}"), "couldn't store the chat's title"),
    }
}

fn group_title(chat: &Chat) -> Option<(TelegramChatId, ChatTitle)> {
    if chat.is_private() || chat.is_channel() {
        return None
    }
    chat.title()
        .map(|title| (chat.id.into(), ChatTitle::new(title.to_owned())))
}

#[cfg(test)]
mod test {
    use serde_json::json;
    use teloxide::dptree;
    use teloxide::dptree::description::Unspecified;
    use teloxide::types::{Me, Message};
    use crate::domain::primitives::chat::{ChatTitle, TelegramChatId};
    use super::title_to_remember;

    const CHAT_ID: i64 = -1001847508954;

    #[test]
    fn our_command_in_a_group_gives_the_title() {
        let title = title_to_remember(group_message("text", "/grow"), me());
        assert_eq!(title, Some(expected("chat")));

        let title = title_to_remember(group_message("text", "/top@DickGrowerBot"), me());
        assert_eq!(title, Some(expected("chat")));
    }

    #[test]
    fn a_rename_gives_the_new_title() {
        let title = title_to_remember(group_message("new_chat_title", "chat"), me());
        assert_eq!(title, Some(expected("chat")));
    }

    #[test]
    fn other_messages_give_nothing() {
        assert_eq!(title_to_remember(group_message("text", "hello"), me()), None);
        assert_eq!(title_to_remember(group_message("text", "/grow@SomeOtherBot"), me()), None);
        assert_eq!(title_to_remember(group_message("text", "/roll 2d6"), me()), None);
    }

    /// The branch in `main.rs` has no endpoint of its own, and the command still has to reach the
    /// branch that answers it.
    #[tokio::test]
    async fn a_branch_without_an_endpoint_lets_the_update_through() {
        let tree = dptree::entry::<&str, Unspecified>()
            .branch(dptree::filter_map(Some::<i32>).inspect(|_: i32| {}))
            .branch(dptree::endpoint(|| async { "answered" }));
        let result = tree.dispatch(dptree::deps![1_i32]).await;
        assert_eq!(result, std::ops::ControlFlow::Break("answered"));
    }

    #[test]
    fn a_private_chat_gives_nothing() {
        let msg: Message = serde_json::from_value(json!({
            "chat": {"id": 1253681278, "first_name": "tester", "type": "private"},
            "date": 1675229140,
            "from": {"first_name": "tester", "id": 1253681278, "is_bot": false},
            "message_id": 5,
            "text": "/grow",
        })).expect("couldn't build the message");
        assert_eq!(title_to_remember(msg, me()), None);
    }

    fn expected(title: &str) -> (TelegramChatId, ChatTitle) {
        (TelegramChatId::new(CHAT_ID), ChatTitle::new(title.to_owned()))
    }

    /// A message of a supergroup titled "chat", with one more field for its content.
    fn group_message(field: &str, value: &str) -> Message {
        let mut json = json!({
            "chat": {"id": CHAT_ID, "title": "chat", "type": "supergroup"},
            "date": 1675229140,
            "from": {"first_name": "tester", "id": 1253681278, "is_bot": false},
            "message_id": 5,
        });
        json[field] = json!(value);
        serde_json::from_value(json).expect("couldn't build the message")
    }

    fn me() -> Me {
        serde_json::from_value(json!({
            "id": 42, "is_bot": true, "first_name": "Dick Grower", "username": "DickGrowerBot",
            "can_join_groups": true, "can_read_all_group_messages": false,
            "supports_inline_queries": true, "can_connect_to_business_account": false,
            "has_main_web_app": false,
        })).expect("couldn't build the bot's own user")
    }
}
