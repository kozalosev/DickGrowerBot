use teloxide::types::MessageId;
use crate::domain::primitives::{AccessHash, DatacenterId};
use crate::domain::primitives::chat::InternalChatId;

#[derive(Debug)]
#[allow(dead_code)]
pub struct InlineMessageIdInfo {
    pub dc_id: DatacenterId,
    pub chat_id: InternalChatId,
    pub message_id: MessageId,
    pub access_hash: AccessHash,
}

impl InlineMessageIdInfo {
    pub fn from_primitive_values(
        dc_id: i32,
        chat_id: i64,
        message_id: i32,
        access_hash: i64,
    ) -> Self {
        Self {
            dc_id: DatacenterId::new(dc_id),
            chat_id: InternalChatId::new(chat_id),
            message_id: MessageId(message_id),
            access_hash: AccessHash::new(access_hash),
        }
    }
}
