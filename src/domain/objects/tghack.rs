use teloxide::types::MessageId;
use crate::domain::primitives::{AccessHash, DatacenterId};
use crate::domain::primitives::chat::TelegramChatId;

#[derive(Debug)]
#[allow(dead_code)]
pub struct InlineMessageIdInfo {
    pub dc_id: DatacenterId,
    /// `None` when the decoded value can't be a genuine supergroup/channel id
    /// (e.g. a legacy/basic group, whose inline ids don't encode the chat at all).
    pub chat_id: Option<TelegramChatId>,
    pub message_id: MessageId,
    pub access_hash: AccessHash,
}

impl InlineMessageIdInfo {
    pub fn from_primitive_values(
        dc_id: i32,
        chat_id: Option<i64>,
        message_id: i32,
        access_hash: i64,
    ) -> Self {
        Self {
            dc_id: DatacenterId::new(dc_id),
            chat_id: chat_id.map(TelegramChatId::new),
            message_id: MessageId(message_id),
            access_hash: AccessHash::new(access_hash),
        }
    }
}
