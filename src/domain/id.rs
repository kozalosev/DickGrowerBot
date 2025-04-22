use crate::number_wrapper;
use teloxide::types::{ChatId, UserId as TeloxideUserId};
use crate::i64_domain;

i64_domain!(InternalChatId);
i64_domain!(UserId);
i64_domain!(LoanId);

impl From<ChatId> for InternalChatId {
    fn from(chat_id: ChatId) -> Self {
        Self(chat_id.0)
    }
}

impl From<TeloxideUserId> for UserId {
    fn from(value: TeloxideUserId) -> Self {
        Self(value.0 as i64)
    }
}

impl InternalChatId {
    pub fn to_string(&self) -> String {}
}
