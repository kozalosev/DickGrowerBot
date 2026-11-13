use std::time::Duration;
use chrono::Utc;
use teloxide::prelude::Requester;
use teloxide::types::{ChatId, Message, MessageId, UserId as TeloxideUserId};
use crate::config::{MessageGroup, SelfDestructionConfig, ThrottledBot, MAX_DELAY};
use crate::handlers::rights;
use crate::handlers::utils::try_resolve_chat_id;
use crate::domain::primitives::{CharCount, LanguageCode};
use crate::domain::objects::ChatCleanupSettings;
use crate::domain::primitives::chat::{ChatIdKind, InlineMessageId, TelegramChatId, TelegramMessageId};
use crate::cache::Cache;
use crate::cleanup::CleanupPolicy;
use crate::repo::{DeletionTarget, MessageKind, NewDeletion, ScheduledDeletions};

/// Estimated time needed to read `char_count` visible characters at `cpm` characters per
/// minute. Returns zero when `cpm` is zero (reading-time adjustment disabled).
fn reading_time(char_count: CharCount, cpm: u64) -> Duration {
    if cpm == 0 {
        return Duration::ZERO
    }
    Duration::from_secs(u64::from(char_count.value()) * 60 / cpm)
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
    cleanup: CleanupPolicy,
    cache: Cache,
    bot_id: TeloxideUserId,
    /// How long what the cache learns about the bot's rights here stays worth believing.
    bot_admin_ttl: Duration,
}

impl SelfDestructionService {
    pub fn new(
        config: SelfDestructionConfig,
        deletions: ScheduledDeletions,
        cleanup: CleanupPolicy,
        cache: Cache,
        bot_id: TeloxideUserId,
        bot_admin_ttl: Duration,
    ) -> Self {
        Self { config, deletions, cleanup, cache, bot_id, bot_admin_ttl }
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
        bot: &ThrottledBot,
        command: &Message,
        sent: &Message,
        group: MessageGroup,
        lang_code: &LanguageCode,
    ) {
        if sent.chat.is_private() {
            return
        }
        let settings = self.cleanup.settings(&sent.chat.id.into()).await;
        let Some(base_delay) = self.config.delay_for_chat(group, &settings) else {
            return
        };
        let char_count = CharCount::of(sent.text().unwrap_or_default());
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

    /// Schedules an inline message to self-destruct on the terms of the chat it went to.
    ///
    /// In a supergroup the `inline_message_id` names the chat and the message, so a bot that may
    /// delete messages there deletes it on the ordinary delays. Anywhere else it can only be
    /// replaced with the placeholder, which the chat keeps for good, so the chat's say in whether
    /// inline messages are touched at all applies, and a chat that said nothing follows the
    /// operator's list rather than its own delays.
    ///
    /// `chat` is `None` where the update doesn't name one: a legacy group without an anchor, or a
    /// chosen result whose `inline_message_id` encodes no chat. Then the bot's own settings apply.
    #[tracing::instrument(skip_all, fields(group = %group, inline_message_id = %inline_message_id, chat_id = ?chat))]
    pub async fn schedule_inline(
        &self,
        inline_message_id: InlineMessageId,
        chat: Option<ChatIdKind>,
        group: MessageGroup,
        char_count: CharCount,
        lang_code: &LanguageCode,
    ) {
        let settings = match &chat {
            Some(chat) => self.cleanup.settings(chat).await,
            None => ChatCleanupSettings::default(),
        };
        let deletable = match try_resolve_chat_id(inline_message_id.value()) {
            Some(chat_id) => rights::may_delete_inline(&self.cache, chat_id.into()).await,
            None => false,
        };
        let Some(base_delay) = self.config.inline_delay(group, &settings, deletable) else {
            return
        };
        let delay = base_delay
            .max(reading_time(char_count, self.config.reading_speed_cpm))
            .min(MAX_DELAY);

        let row = NewDeletion {
            target: DeletionTarget::InlineMessage(inline_message_id),
            kind: MessageKind::Inline,
            group,
            lang_code: lang_code.clone(),
            fire_after: Utc::now() + delay,
        };
        tracing::debug!(delay = ?delay, deletable = %deletable, "scheduling the self-destruction of an inline message");
        self.deletions.schedule(&[row]).await
            .unwrap_or_else(|e| tracing::error!(error = format!("{e:#}"), "couldn't schedule the self-destruction"));
    }

    /// Calls off the self-destruction of an inline message that has become worth keeping — an
    /// application answered before it could expire.
    #[tracing::instrument(skip_all, fields(inline_message_id = %inline_message_id))]
    pub async fn cancel_inline(&self, inline_message_id: InlineMessageId) {
        if !self.config.enabled() {
            return
        }
        self.cancel(DeletionTarget::InlineMessage(inline_message_id)).await
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
    ///   exists to prevent. Only there is Telegram asked, and only when nothing is known.
    ///
    /// [`DeletionMode::Enabled`]: crate::config::DeletionMode::Enabled
    /// [`DeletionMode::OnlyWithCommand`]: crate::config::DeletionMode::OnlyWithCommand
    async fn may_delete_commands(&self, bot: &ThrottledBot, chat_id: ChatId) -> bool {
        if let Some(known) = rights::cached_deletion_right(&self.cache, chat_id).await {
            return known
        }
        if !self.config.requires_command() {
            return true
        }
        self.ask_about_the_rights(bot, chat_id).await
    }

    /// The same question without the guessing, for the one caller that has to know: the picker of
    /// `/cleanup`, which warns an admin whose chat is about to lose the answers while keeping the
    /// commands. A press is rare enough to be worth the request, and the answer it stores serves
    /// every message that follows.
    pub async fn may_delete_here(&self, bot: &ThrottledBot, chat_id: ChatId) -> bool {
        match rights::cached_deletion_right(&self.cache, chat_id).await {
            Some(known) => known,
            None => self.ask_about_the_rights(bot, chat_id).await,
        }
    }

    /// Asks Telegram and remembers the answer. A failed request answers "no": keeping both
    /// messages is the harmless outcome.
    async fn ask_about_the_rights(&self, bot: &ThrottledBot, chat_id: ChatId) -> bool {
        let is_admin = match bot.get_chat_member(chat_id, self.bot_id).await {
            Ok(member) => member.can_delete_messages(),
            Err(e) => {
                tracing::warn!(error = %e, "couldn't ask Telegram about the bot's rights in the chat");
                return false
            }
        };
        rights::remember_deletion_right(&self.cache, chat_id, is_admin, self.bot_admin_ttl).await;
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
    use sqlx::{Pool, Postgres};
    use crate::config::{CacheConfig, CacheMode, DeletionMode, InlineGroups};
    use crate::handlers::utils::tghack;
    use crate::repo::Chats;
    use crate::repo::test::fresh_db;
    use super::*;

    #[test]
    fn reading_time_scales_with_length() {
        // 1000 chars/min => 1000 chars take a minute, 2000 take two.
        assert_eq!(reading_time(CharCount::new(0), 1000), Duration::ZERO);
        assert_eq!(reading_time(CharCount::new(1000), 1000), Duration::from_mins(1));
        assert_eq!(reading_time(CharCount::new(2000), 1000), Duration::from_mins(2));
    }

    #[test]
    fn reading_time_zero_speed_disables_adjustment() {
        assert_eq!(reading_time(CharCount::new(5000), 0), Duration::ZERO);
    }

    /// The bot opens no group for the placeholder, so only a message it can delete is scheduled.
    #[tokio::test]
    async fn only_a_deletable_inline_message_ignores_the_placeholder_policy() {
        const CHAT_ID: i64 = -1001100294568;
        let db = fresh_db().await;
        let cache = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        let service = service(&db, &cache);
        let lang_code = LanguageCode::new("en".to_owned());
        let schedule = |id: String| service.schedule_inline(InlineMessageId::new(id), None, MessageGroup::Report,
                                                            CharCount::new(0), &lang_code);

        schedule("AgAAAOEcAABzXwsRJ0Cs2A".to_owned()).await;
        assert_eq!(scheduled(&db).await, 0, "an id that names no chat can only be shrunk");

        schedule(tghack::inline_message_id_of(CHAT_ID)).await;
        assert_eq!(scheduled(&db).await, 1, "a supergroup's message is deleted on the ordinary delay");

        rights::remember_deletion_right(&cache, ChatId(CHAT_ID - 1), false, Duration::from_hours(1)).await;
        schedule(tghack::inline_message_id_of(CHAT_ID - 1)).await;
        assert_eq!(scheduled(&db).await, 1, "a chat known to refuse can only be shrunk");
    }

    fn service(db: &Pool<Postgres>, cache: &Cache) -> SelfDestructionService {
        let config = SelfDestructionConfig {
            mode: DeletionMode::Enabled,
            report: Duration::from_mins(2),
            inline_groups: InlineGroups::default(),
            ..Default::default()
        };
        let cleanup = CleanupPolicy::new(Chats::new(db.clone(), Default::default()), cache.clone(), Duration::from_hours(1));
        SelfDestructionService::new(config, ScheduledDeletions::new(db.clone()), cleanup, cache.clone(),
                                    TeloxideUserId(1), Duration::from_hours(1))
    }

    async fn scheduled(db: &Pool<Postgres>) -> i64 {
        sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM Scheduled_Message_Deletions"#)
            .fetch_one(db)
            .await.expect("couldn't count the scheduled deletions")
    }
}
