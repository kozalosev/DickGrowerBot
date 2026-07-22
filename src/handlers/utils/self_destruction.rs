use std::time::Duration;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::payloads::EditMessageTextSetters;
use teloxide::prelude::Requester;
use teloxide::types::Message;
use teloxide::types::ParseMode::Html;
use crate::config::{MessageGroup, SelfDestructionConfig};
use crate::domain::primitives::LanguageCode;

/// Estimated time needed to read `char_count` visible characters at `cpm` characters per
/// minute. Returns zero when `cpm` is zero (reading-time adjustment disabled).
fn reading_time(char_count: usize, cpm: u64) -> Duration {
    if cpm == 0 {
        return Duration::ZERO
    }
    Duration::from_secs(char_count as u64 * 60 / cpm)
}

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
    /// so we never clean them up. The delay is the larger of the group's configured base
    /// delay and the time needed to read the message, so long messages linger long enough.
    ///
    /// If a warning grace period is configured, the message is first edited into a
    /// localized "will be deleted in N seconds" notice (in `lang_code`) and only removed
    /// once that period elapses. Never blocks — the wait-edit-delete runs in a spawned task.
    pub fn schedule(&self, bot: &Bot, msg: &Message, group: MessageGroup, lang_code: &LanguageCode) {
        if msg.chat.is_private() {
            return
        }
        let Some(base_delay) = self.config.delay_for(group) else {
            return
        };
        let char_count = msg.text().map_or(0, |t| t.chars().count());
        let delay = base_delay.max(reading_time(char_count, self.config.reading_speed_cpm));

        let bot = bot.clone();
        let chat_id = msg.chat.id;
        let message_id = msg.id;
        let warning = self.config.warning;
        let lang_code = lang_code.clone();
        log::debug!("self-destruction: scheduling deletion of {group} message {message_id} in chat {chat_id} in {delay:?}");
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;

            if !warning.is_zero() {
                let notice = t!("self_destruction.warning", locale = &lang_code, seconds = warning.as_secs());
                log::debug!("self-destruction: warning before deleting {group} message {message_id} in chat {chat_id}");
                if let Err(e) = bot.edit_message_text(chat_id, message_id, notice).parse_mode(Html).await {
                    log::warn!("self-destruction: couldn't edit {group} message {message_id} in chat {chat_id}: {e}");
                }
                tokio::time::sleep(warning).await;
            }

            log::debug!("self-destruction: sending delete request for {group} message {message_id} in chat {chat_id}");
            if let Err(e) = bot.delete_message(chat_id, message_id).await {
                log::warn!("self-destruction: couldn't delete {group} message {message_id} in chat {chat_id}: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_time_scales_with_length() {
        // 1000 chars/min => 1000 chars take a minute, 2000 take two.
        assert_eq!(reading_time(0, 1000), Duration::ZERO);
        assert_eq!(reading_time(1000, 1000), Duration::from_secs(60));
        assert_eq!(reading_time(2000, 1000), Duration::from_secs(120));
    }

    #[test]
    fn reading_time_zero_speed_disables_adjustment() {
        assert_eq!(reading_time(5000, 0), Duration::ZERO);
    }
}
