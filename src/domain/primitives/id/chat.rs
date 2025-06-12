use teloxide::types::ChatId;
use crate::*;

id! {
    InternalChatId,
    TelegramChatId
}

impl From<ChatId> for TelegramChatId {
    fn from(chat_id: ChatId) -> Self {
        Self(chat_id.0)
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

impl From<ChatId> for ChatIdPartiality {
    fn from(value: ChatId) -> Self {
        Self::Specific(ChatIdKind::ID(value))
    }
}

impl From<String> for ChatIdPartiality {
    fn from(value: String) -> Self {
        Self::Specific(ChatIdKind::Instance(value))
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
    pub id: ChatId,
    pub instance: String,
}

impl ChatIdFull {
    #[allow(clippy::wrong_self_convention)]
    pub fn to_partiality(self, query_source: ChatIdSource) -> ChatIdPartiality {
        ChatIdPartiality::Both(self, query_source)
    }
}

#[derive(Debug, derive_more::Display, Clone, Eq, PartialEq, Hash)]
pub enum ChatIdKind {
    ID(ChatId),
    Instance(String)
}

impl From<ChatId> for ChatIdKind {
    fn from(value: ChatId) -> Self {
        ChatIdKind::ID(value)
    }
}

impl From<String> for ChatIdKind {
    fn from(value: String) -> Self {
        ChatIdKind::Instance(value)
    }
}

impl ChatIdKind {
    pub fn value(&self) -> String {
        match self {
            ChatIdKind::ID(id) => id.0.to_string(),
            ChatIdKind::Instance(instance) => instance.to_owned(),
        }
    }
}


