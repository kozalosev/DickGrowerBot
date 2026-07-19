mod macros;
pub mod chat;

use teloxide::types::{UserId as TeloxideUserId, User as TeloxideUser};
use domain_types_macro::domain_type;
use crate::*;

id!(LoanId);
positive_id!(UserId);

#[domain_type]
struct DatacenterId(i32);

impl From<TeloxideUserId> for UserId {
    fn from(value: TeloxideUserId) -> Self {
        UserId::new(value.0 as i64).expect("Telegram user id must be positive")
    }
}

impl From<UserId> for TeloxideUserId {
    fn from(value: UserId) -> Self {
        TeloxideUserId(value.value() as u64)
    }
}

impl From<&TeloxideUser> for UserId {
    fn from(value: &TeloxideUser) -> Self {
        Self::from(value.id)
    }
}

impl PartialEq<TeloxideUserId> for UserId {
    fn eq(&self, other: &TeloxideUserId) -> bool {
        self.0 == (other.0 as i64)
    }
}
