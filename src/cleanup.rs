use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::time::Instant;
use crate::domain::enums::MessageGroup;
use crate::domain::objects::ChatCleanupSettings;
use crate::domain::primitives::DelayMinutes;
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality};
use crate::metrics;
use crate::repo::Chats;

/// Which of its answers each chat has the bot clean up, cached in front of the database.
///
/// The lookup happens on the answering path, once per message the bot sends into a group, so it
/// has to be cheap. Writes go through here too and refresh the entry, so an admin who flips a
/// switch sees it take effect at once rather than after the TTL.
#[derive(Clone)]
pub struct CleanupPolicy {
    chats: Chats,
    cache: Arc<Mutex<HashMap<ChatIdKind, CachedSettings>>>,
    ttl: Duration,
}

#[derive(Clone)]
struct CachedSettings {
    settings: ChatCleanupSettings,
    at: Instant,
}

impl CleanupPolicy {
    pub fn new(ttl: Duration, chats: Chats) -> Self {
        Self {
            chats,
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    /// Read-through TTL cache over [`Chats::get_cleanup_settings`].
    ///
    /// A failed read reports the chat as having decided nothing, so a database the bot can't reach
    /// leaves its messages behaving the way they were configured to.
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn settings(&self, chat_id: &ChatIdKind) -> ChatCleanupSettings {
        let now = Instant::now();
        if let Some(cached) = self.cache().get(chat_id)
            .filter(|cached| now.duration_since(cached.at) <= self.ttl)
        {
            metrics::CHAT_CLEANUP.cache_hit();
            return cached.settings.clone();
        }

        metrics::CHAT_CLEANUP.db_query();
        let settings = self.chats.get_cleanup_settings(chat_id).await
            .unwrap_or_else(|e| {
                tracing::warn!(error = format!("{e:#}"), "couldn't fetch the cleanup settings of the chat");
                ChatCleanupSettings::default()
            });
        self.cache().insert(chat_id.clone(), CachedSettings { settings: settings.clone(), at: now });
        settings
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
        self.cache().remove(&kind);
        self.chats.get_cleanup_settings(&kind).await
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<ChatIdKind, CachedSettings>> {
        self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
