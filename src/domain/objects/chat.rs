use teloxide::types::ChatId;
use crate::domain::primitives::chat::InternalChatId;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Chat {
    pub internal_id: InternalChatId,
    pub chat_id: Option<ChatId>,
    pub chat_instance: Option<String>,
}
