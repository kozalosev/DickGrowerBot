use teloxide::Bot;
use teloxide::prelude::Requester;
use teloxide::types::Message;
use crate::config::{MessageGroup, SelfDestructionConfig};

/// Best-effort, in-memory scheduler that deletes the bot's own messages after a
/// per-group delay (see [`MessageGroup`]).
///
/// This is the proof-of-concept from issue #49 (sub-issue #126): deletions are spawned
/// as detached tasks and are **not** persisted, so any still-pending deletions are lost
/// on restart. That trade-off is acceptable for the short-lived groups it handles
/// (`Notice`, `Report`). The DB-backed, restart-surviving version — plus long-living
/// applications, command deletion and replacing inline messages with placeholders — is
/// tracked in #127.
#[derive(Clone)]
pub struct SelfDestructionService {
    config: SelfDestructionConfig,
}

impl SelfDestructionService {
    pub fn new(config: SelfDestructionConfig) -> Self {
        Self { config }
    }

    /// Schedule `msg` for deletion according to its group. Does nothing if the group is
    /// permanent (zero delay) or the message is in a private chat — 1:1 chats aren't noisy,
    /// so we never clean them up. Never blocks — the wait-and-delete runs in a spawned task.
    pub fn schedule(&self, bot: &Bot, msg: &Message, group: MessageGroup) {
        if msg.chat.is_private() {
            return
        }
        let Some(delay) = self.config.delay_for(group) else {
            return
        };
        let bot = bot.clone();
        let chat_id = msg.chat.id;
        let message_id = msg.id;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                log::warn!("self-destruction: couldn't delete message {message_id} in chat {chat_id}: {e}");
            }
        });
    }
}
