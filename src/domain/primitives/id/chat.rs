use teloxide::types::{ChatId, MessageId, ThreadId};
use domain_types_macro::domain_type;
use crate::*;

id! {
    InternalChatId,
    TelegramChatId
}

#[domain_type]
struct TelegramChatInstanceId(String);

/// A forum topic of a supergroup, identified by the message that created it.
///
/// Telegram omits `message_thread_id` for the General topic, so [`TopicId::GENERAL`] stands for
/// its absence.
#[domain_type]
struct TopicId(i32);

impl TopicId {
    /// The topic every forum has and no one created — the one Telegram reports as no topic at all.
    pub const GENERAL: Self = Self(0);
}

/// A message inside a chat. Unique for that chat only, which is why nothing addresses a message
/// by it alone.
#[domain_type]
struct TelegramMessageId(i32);

/// The opaque handle Telegram gives an inline message. It's the only way to address one — such a
/// message has no chat and no message id of its own, and can never be deleted, only edited.
#[domain_type]
struct InlineMessageId(String);

impl From<MessageId> for TelegramMessageId {
    fn from(message_id: MessageId) -> Self {
        Self(message_id.0)
    }
}

impl From<TelegramMessageId> for MessageId {
    fn from(message_id: TelegramMessageId) -> Self {
        Self(message_id.value())
    }
}

impl From<ThreadId> for TopicId {
    fn from(thread_id: ThreadId) -> Self {
        Self(thread_id.0.0)
    }
}

impl From<Option<ThreadId>> for TopicId {
    fn from(thread_id: Option<ThreadId>) -> Self {
        thread_id.map(Self::from).unwrap_or(Self::GENERAL)
    }
}

impl From<TopicId> for ThreadId {
    fn from(topic_id: TopicId) -> Self {
        Self(MessageId(topic_id.value()))
    }
}

impl From<ChatId> for TelegramChatId {
    fn from(chat_id: ChatId) -> Self {
        Self(chat_id.0)
    }
}

impl From<TelegramChatId> for ChatId {
    fn from(chat_id: TelegramChatId) -> Self {
        Self(chat_id.value())
    }
}

#[derive(derive_more::Display, Debug, Default, Copy, Clone)]
pub enum ChatIdSource {
    InlineQuery,
    #[default] Database,
}

#[derive(derive_more::Display, Debug, Clone)]
pub enum ChatIdPartiality {
    #[display("ChatIdPartiality::Both({_0}, {_1})")]
    Both(ChatIdFull, ChatIdSource),
    #[display("ChatIdPartiality::Specific({_0})")]
    Specific(ChatIdKind)
}

impl From<TelegramChatId> for ChatIdPartiality {
    fn from(value: TelegramChatId) -> Self {
        Self::Specific(ChatIdKind::ID(value))
    }
}

impl From<ChatId> for ChatIdPartiality {
    fn from(value: ChatId) -> Self {
        Self::from(TelegramChatId::from(value))
    }
}

impl From<String> for ChatIdPartiality {
    fn from(value: String) -> Self {
        Self::Specific(ChatIdKind::Instance(TelegramChatInstanceId::new(value)))
    }
}

impl From<ChatIdKind> for ChatIdPartiality {
    fn from(value: ChatIdKind) -> Self {
        Self::Specific(value)
    }
}

impl ChatIdPartiality {
    pub fn kind(&self) -> ChatIdKind {
        match self {
            ChatIdPartiality::Both(ChatIdFull { id, instance }, qs) => match qs {
                ChatIdSource::Database => ChatIdKind::ID(*id),
                ChatIdSource::InlineQuery => ChatIdKind::Instance(instance.clone()),
            }
            ChatIdPartiality::Specific(kind) => kind.clone()
        }
    }
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("ChatIdFull({id}, {instance})")]
pub struct ChatIdFull {
    pub id: TelegramChatId,
    pub instance: TelegramChatInstanceId,
}

impl ChatIdFull {
    #[allow(clippy::wrong_self_convention)]
    pub fn to_partiality(self, query_source: ChatIdSource) -> ChatIdPartiality {
        ChatIdPartiality::Both(self, query_source)
    }
}

#[derive(Debug, derive_more::Display, Clone, Eq, PartialEq, Hash)]
pub enum ChatIdKind {
    ID(TelegramChatId),
    Instance(TelegramChatInstanceId)
}

impl From<ChatId> for ChatIdKind {
    fn from(value: ChatId) -> Self {
        ChatIdKind::ID(TelegramChatId::from(value))
    }
}

impl From<TelegramChatId> for ChatIdKind {
    fn from(value: TelegramChatId) -> Self {
        ChatIdKind::ID(value)
    }
}

impl From<String> for ChatIdKind {
    fn from(value: String) -> Self {
        ChatIdKind::Instance(TelegramChatInstanceId::new(value))
    }
}

impl ChatIdKind {
    pub fn value(&self) -> String {
        match self {
            ChatIdKind::ID(id) => id.0.to_string(),
            ChatIdKind::Instance(instance) => instance.to_string(),
        }
    }
}


