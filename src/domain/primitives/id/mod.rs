mod macros;
pub mod chat;

use teloxide::types::{UserId as TeloxideUserId};
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
