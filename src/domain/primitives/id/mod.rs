mod macros;
pub mod chat;

use teloxide::types::{UserId as TeloxideUserId, User as TeloxideUser};
use domain_types_macro::domain_type;
use crate::*;

id! {
    UserId,
    LoanId
}

#[domain_type]
struct DatacenterId(i32);

impl From<TeloxideUserId> for UserId {
    fn from(value: TeloxideUserId) -> Self {
        Self(value.0 as i64)
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
