mod macros;
pub mod chat;

use teloxide::types::{UserId as TeloxideUserId, User as TeloxideUser};
use domain_types_macro::domain_type;
use crate::*;

id!(
    LoanId,
    UserId,
    ScheduledDeletionId
);

/// Which of Telegram's datacenters a message lives in — a number from 1 to 5, read out of an
/// `inline_message_id` and never stored.
#[domain_type]
struct DatacenterId(u8);

impl From<TeloxideUserId> for UserId {
    fn from(value: TeloxideUserId) -> Self {
        UserId::new(value.0)
    }
}

impl From<UserId> for TeloxideUserId {
    fn from(value: UserId) -> Self {
        TeloxideUserId(value.value())
    }
}

impl From<&TeloxideUser> for UserId {
    fn from(value: &TeloxideUser) -> Self {
        Self::from(value.id)
    }
}

impl PartialEq<TeloxideUserId> for UserId {
    fn eq(&self, other: &TeloxideUserId) -> bool {
        self.0 == other.0
    }
}
