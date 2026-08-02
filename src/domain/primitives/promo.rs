use domain_types_macro::domain_type;
use crate::{positive_number, signed_number};

#[domain_type]
struct PromoCode(String);

signed_number!(PromoBonus, i32);
positive_number!(PromoCapacity, i32);
