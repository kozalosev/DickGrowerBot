use std::time::Duration;
use derive_more::Constructor;
use crate::cache::{Cache, CacheKey};
use crate::domain::objects::AllowedTopics;
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality, TopicId};
use crate::metrics;
use crate::repo::Chats;

/// Keyed by chat, because a forum's topics are its own.
#[derive(derive_more::Display)]
#[display("chat:{}:topics", _0.qualified())]
struct TopicsKey(ChatIdKind);

impl CacheKey for TopicsKey {}

/// Which forum topics each chat lets the bot work in, cached in front of the database.
///
/// The gate runs on every command of every group, so the lookup has to be cheap. Writes go
/// through here too and forget the entry, so an admin who flips the setting sees it take effect at
/// once — on every instance of the bot, rather than on the one that was asked.
#[derive(Clone, Constructor)]
pub struct TopicPolicy {
    chats: Chats,
    cache: Cache,
    ttl: Duration,
}

impl TopicPolicy {
    /// Read-through cache over [`Chats::get_allowed_topics`].
    ///
    /// A failed read reports the chat as unrestricted: a database the bot can't reach must not
    /// lock a chat out of it.
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn allowed(&self, chat_id: &ChatIdKind) -> AllowedTopics {
        self.cache.read_through(TopicsKey(chat_id.clone()), self.ttl, &metrics::CHAT_TOPICS, || async {
            self.chats.get_allowed_topics(chat_id).await
                .unwrap_or_else(|e| {
                    tracing::warn!(error = format!("{e:#}"), "couldn't fetch the allowed topics of the chat");
                    AllowedTopics::default()
                })
        }).await
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
        self.cache.remove(TopicsKey(kind.clone())).await;
        self.chats.get_allowed_topics(&kind).await
    }
}
