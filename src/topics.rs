use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::time::Instant;
use crate::config::IntegrationsConfig;
use crate::domain::objects::AllowedTopics;
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality, TopicId};
use crate::metrics;
use crate::repo::Chats;

/// Which forum topics each chat lets the bot work in, cached in front of the database.
///
/// The gate runs on every command of every group, so the lookup has to be cheap. Writes go
/// through here too and refresh the entry, so an admin who flips the setting sees it take effect
/// at once rather than after the TTL.
#[derive(Clone)]
pub struct TopicPolicy {
    chats: Chats,
    cache: Arc<Mutex<HashMap<ChatIdKind, CachedTopics>>>,
    ttl: Duration,
}

#[derive(Clone)]
struct CachedTopics {
    topics: AllowedTopics,
    at: Instant,
}

impl TopicPolicy {
    pub fn new(config: &IntegrationsConfig, chats: Chats) -> Self {
        Self {
            chats,
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(config.chat_topics_cache_time_secs),
        }
    }

    /// Read-through TTL cache over [`Chats::get_allowed_topics`].
    ///
    /// A failed read reports the chat as unrestricted: a database the bot can't reach must not
    /// lock a chat out of it.
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn allowed(&self, chat_id: &ChatIdKind) -> AllowedTopics {
        let now = Instant::now();
        if let Some(cached) = self.cache().get(chat_id)
            .filter(|cached| now.duration_since(cached.at) <= self.ttl)
        {
            metrics::CHAT_TOPICS.cache_hit();
            return cached.topics.clone();
        }

        metrics::CHAT_TOPICS.db_query();
        let topics = self.chats.get_allowed_topics(chat_id).await
            .unwrap_or_else(|e| {
                tracing::warn!(error = format!("{e:#}"), "couldn't fetch the allowed topics of the chat");
                AllowedTopics::default()
            });
        self.cache().insert(chat_id.clone(), CachedTopics { topics: topics.clone(), at: now });
        topics
    }

    /// Whether the bot may answer in this topic of this chat.
    pub async fn allows(&self, chat_id: &ChatIdKind, topic: TopicId) -> bool {
        self.allowed(chat_id).await.allows(topic)
    }

    pub async fn allow_topic(&self, chat_id: &ChatIdPartiality, topic: TopicId) -> anyhow::Result<AllowedTopics> {
        self.chats.allow_topic(chat_id, topic).await?;
        self.refresh(chat_id).await
    }

    pub async fn forbid_topic(
        &self,
        chat_id: &ChatIdPartiality,
        topic: TopicId,
    ) -> anyhow::Result<AllowedTopics> {
        self.chats.forbid_topic(chat_id, topic).await?;
        self.refresh(chat_id).await
    }

    pub async fn allow_all_topics(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<AllowedTopics> {
        self.chats.allow_all_topics(chat_id).await?;
        self.refresh(chat_id).await
    }

    /// Drops the cached entry and reads the setting back, so the caller renders what the database
    /// actually holds rather than what it tried to write.
    async fn refresh(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<AllowedTopics> {
        let kind = chat_id.kind();
        self.cache().remove(&kind);
        self.chats.get_allowed_topics(&kind).await
    }

    fn cache(&self) -> MutexGuard<'_, HashMap<ChatIdKind, CachedTopics>> {
        self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
