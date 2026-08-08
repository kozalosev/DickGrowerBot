use std::time::Duration;
use chrono::Utc;
use teloxide::Bot;
use teloxide::prelude::Requester;
use teloxide::types::{ChatId, Message, MessageId, UserId as TeloxideUserId};
use crate::config::{MessageGroup, SelfDestructionConfig, MAX_DELAY};
use crate::domain::primitives::LanguageCode;
use crate::domain::primitives::chat::{InlineMessageId, TelegramChatId, TelegramMessageId};
use crate::cache::Cache;
use crate::repo::{DeletionTarget, MessageKind, NewDeletion, ScheduledDeletions};

/// Estimated time needed to read `char_count` visible characters at `cpm` characters per
/// minute. Returns zero when `cpm` is zero (reading-time adjustment disabled).
fn reading_time(char_count: usize, cpm: u64) -> Duration {
    if cpm == 0 {
        return Duration::ZERO
    }
    Duration::from_secs(char_count as u64 * 60 / cpm)
}

/// Writes down the messages that are to disappear on their own (see [`MessageGroup`]) and hands
/// them to the worker in `scheduler::deletions`, which comes back for them when their time is up.
///
/// Everything goes through the database, short-lived groups included, so a restart between an
/// answer and its deletion loses nothing. Scheduling costs one insert on the answering path; the
/// deletions themselves happen elsewhere.
#[derive(Clone)]
pub struct SelfDestructionService {
    config: SelfDestructionConfig,
    deletions: ScheduledDeletions,
    cache: Cache,
    bot_id: TeloxideUserId,
}

impl SelfDestructionService {
    pub fn new(
        config: SelfDestructionConfig,
        deletions: ScheduledDeletions,
        cache: Cache,
        bot_id: TeloxideUserId,
    ) -> Self {
        Self { config, deletions, cache, bot_id }
    }

    /// Schedules the answer `sent` and, when the configuration and the bot's rights allow it, the
    /// `command` that caused it. Does nothing if the group is permanent (zero delay) or the chat is
    /// private — 1:1 chats aren't noisy, so we never clean them up. The delay is the larger of the
    /// group's configured base delay and the time needed to read the answer, so long messages
    /// linger long enough.
    ///
    /// A failure to write the rows is logged and swallowed: the answer has already been sent, and
    /// a message that outlives its welcome is a far smaller problem than a command that fails.
    #[tracing::instrument(skip_all, fields(group = %group, chat_id = %sent.chat.id, message_id = %sent.id))]
    pub async fn schedule(
        &self,
        bot: &Bot,
        command: &Message,
        sent: &Message,
        group: MessageGroup,
        lang_code: &LanguageCode,
    ) {
        if sent.chat.is_private() {
            return
        }
        let Some(base_delay) = self.config.delay_for(group) else {
            return
        };
        let char_count = sent.text().map_or(0, |t| t.chars().count());
        let delay = base_delay
            .max(reading_time(char_count, self.config.reading_speed_cpm))
            .min(MAX_DELAY);

        let with_command = self.config.deletes_commands()
            && self.may_delete_commands(bot, sent.chat.id).await;
        if self.config.requires_command() && !with_command {
            tracing::debug!("the answer is kept because its command can't be deleted");
            return
        }

        let fire_after = Utc::now() + delay;
        let mut rows = vec![NewDeletion {
            target: target_of(sent),
            kind: MessageKind::Reply,
            group,
            lang_code: lang_code.clone(),
            fire_after,
        }];
        if with_command {
            // The command outlives the answer by the grace period so that both disappear at once:
            // it can't be edited into the warning the answer shows meanwhile.
            rows.push(NewDeletion {
                target: target_of(command),
                kind: MessageKind::Command,
                group,
                lang_code: lang_code.clone(),
                fire_after: fire_after + self.config.warning,
            });
        }

        tracing::debug!(delay = ?delay, with_command = %with_command, "scheduling the self-destruction of a message");
        self.deletions.schedule(&rows).await
            .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "couldn't schedule the self-destruction"));
    }

    /// Schedules an inline message to be replaced with the placeholder. Applies only to the groups
    /// the placeholder is enabled for — an inline message can never be deleted, so the whole chat
    /// keeps whatever is put there instead, and that is worth being conservative about.
    #[tracing::instrument(skip_all, fields(group = %group, inline_message_id = %inline_message_id))]
    pub async fn schedule_inline(&self, inline_message_id: &str, group: MessageGroup, lang_code: &LanguageCode) {
        if !self.config.inline_groups.contains(group) {
            return
        }
        let Some(delay) = self.config.delay_for(group) else {
            return
        };

        let row = NewDeletion {
            target: DeletionTarget::InlineMessage(InlineMessageId::new(inline_message_id.to_owned())),
            kind: MessageKind::Inline,
            group,
            lang_code: lang_code.clone(),
            fire_after: Utc::now() + delay,
        };
        tracing::debug!(delay = ?delay, "scheduling the placeholdering of an inline message");
        self.deletions.schedule(&[row]).await
            .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "couldn't schedule the placeholdering"));
    }

    /// Calls off the placeholdering of an inline message that has become worth keeping — an
    /// application answered before it could expire.
    #[tracing::instrument(skip_all, fields(inline_message_id = %inline_message_id))]
    pub async fn cancel_inline(&self, inline_message_id: &str) {
        if self.config.inline_groups.is_empty() {
            return
        }
        self.cancel(DeletionTarget::InlineMessage(InlineMessageId::new(inline_message_id.to_owned()))).await
    }

    /// The same for a message of a chat: an application that has just been answered is no longer
    /// one, and its outcome is worth as much as any other event.
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, message_id = %message_id))]
    pub async fn cancel_message(&self, chat_id: ChatId, message_id: MessageId) {
        if !self.config.enabled() {
            return
        }
        self.cancel(DeletionTarget::ChatMessage {
            chat_id: TelegramChatId::from(chat_id),
            message_id: TelegramMessageId::from(message_id),
        }).await
    }

    async fn cancel(&self, target: DeletionTarget) {
        self.deletions.cancel(&target).await
            .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "couldn't cancel a self-destruction"));
    }

    /// Whether the bot may delete other members' messages in this chat.
    ///
    /// The answer comes from the cache, which `handlers::rights` fills from every `my_chat_member`
    /// update and the worker corrects on a refusal. When the cache knows nothing, what to do
    /// depends on the mode, because the two modes pay different prices for a wrong guess:
    ///
    /// * [`DeletionMode::Enabled`] guesses "yes" and finds out by trying. A refusal costs one
    ///   request, marks the row `failed` and teaches the cache, and no one sees any of it.
    /// * [`DeletionMode::OnlyWithCommand`] can't guess. The answer is deleted *before* its command,
    ///   so a refusal would leave the command sitting alone in the chat — the very thing the mode
    ///   exists to prevent. Only here is Telegram asked, and only when nothing is known.
    ///
    /// A failed request answers "no": keeping both messages is the harmless outcome.
    ///
    /// [`DeletionMode::Enabled`]: crate::config::DeletionMode::Enabled
    /// [`DeletionMode::OnlyWithCommand`]: crate::config::DeletionMode::OnlyWithCommand
    async fn may_delete_commands(&self, bot: &Bot, chat_id: ChatId) -> bool {
        if let Some(known) = self.cache.bot_admin(chat_id).await {
            return known
        }
        if !self.config.requires_command() {
            return true
        }

        let is_admin = match bot.get_chat_member(chat_id, self.bot_id).await {
            Ok(member) => member.can_delete_messages(),
            Err(e) => {
                tracing::warn!(error = %e, "couldn't ask Telegram about the bot's rights in the chat");
                return false
            }
        };
        self.cache.set_bot_admin(chat_id, is_admin).await;
        is_admin
    }
}

fn target_of(msg: &Message) -> DeletionTarget {
    DeletionTarget::ChatMessage {
        chat_id: TelegramChatId::from(msg.chat.id),
        message_id: TelegramMessageId::from(msg.id),
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
