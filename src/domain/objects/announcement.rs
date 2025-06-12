use crate::domain::primitives::{Counter, TextHash};
use crate::domain::primitives::chat::InternalChatId;

#[derive(sqlx::FromRow)]
pub struct Announcement {
    pub chat_id: InternalChatId,
    pub hash: TextHash,
    pub times_shown: Counter,
}
