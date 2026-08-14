use std::time::Duration;
use derive_more::Constructor;
use crate::cache::{Cache, CacheKey};
use crate::domain::enums::MessageGroup;
use crate::domain::objects::ChatCleanupSettings;
use crate::domain::primitives::DelayMinutes;
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality};
use crate::metrics;
use crate::repo::Chats;

/// Keyed by chat, because each of them decides for itself.
#[derive(derive_more::Display)]
#[display("chat:{}:cleanup", _0.qualified())]
struct CleanupKey(ChatIdKind);

impl CacheKey for CleanupKey {}

/// Which of its answers each chat has the bot clean up, cached in front of the database.
///
/// The lookup happens on the answering path, once per message the bot sends into a group, so it
/// has to be cheap. Writes go through here too and forget the entry, so an admin who flips a switch
/// sees it take effect at once — on every instance of the bot, rather than on the one that was
/// asked.
#[derive(Clone, Constructor)]
pub struct CleanupPolicy {
    chats: Chats,
    cache: Cache,
    ttl: Duration,
}

impl CleanupPolicy {
    /// Read-through cache over [`Chats::get_cleanup_settings`].
    ///
    /// A failed read reports the chat as having decided nothing, so a database the bot can't reach
    /// leaves its messages behaving the way they were configured to.
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn settings(&self, chat_id: &ChatIdKind) -> ChatCleanupSettings {
        self.cache.read_through(CleanupKey(chat_id.clone()), self.ttl, &metrics::CHAT_CLEANUP, || async {
            self.chats.get_cleanup_settings(chat_id).await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = format!("{e:#}"), "couldn't fetch the cleanup settings of the chat");
                    ChatCleanupSettings::default()
                })
        }).await
    }

    pub async fn set_group(
        &self,
        chat_id: &ChatIdPartiality,
        group: MessageGroup,
        minutes: DelayMinutes,
    ) -> anyhow::Result<ChatCleanupSettings> {
        self.chats.set_cleanup_group(chat_id, group, minutes).await?;
        self.refresh(chat_id).await
    }

    pub async fn set_inline(
        &self,
        chat_id: &ChatIdPartiality,
        compress: bool,
    ) -> anyhow::Result<ChatCleanupSettings> {
        self.chats.set_cleanup_inline(chat_id, compress).await?;
        self.refresh(chat_id).await
    }

    /// Takes back the chat's choice about one group, leaving the rest of them as they are.
    pub async fn follow_the_bot(
        &self,
        chat_id: &ChatIdPartiality,
        group: MessageGroup,
    ) -> anyhow::Result<ChatCleanupSettings> {
        self.chats.forget_cleanup_group(chat_id, group).await?;
        self.refresh(chat_id).await
    }

    pub async fn reset(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<ChatCleanupSettings> {
        self.chats.reset_cleanup(chat_id).await?;
        self.refresh(chat_id).await
    }

    /// Drops the cached entry and reads the setting back, so the caller renders what the database
    /// actually holds rather than what it tried to write.
    async fn refresh(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<ChatCleanupSettings> {
        let kind = chat_id.kind();
        self.cache.remove(CleanupKey(kind.clone())).await;
        self.chats.get_cleanup_settings(&kind).await
    }
}
