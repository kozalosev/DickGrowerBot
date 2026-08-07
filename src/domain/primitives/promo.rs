use domain_types_macro::domain_type;
use crate::number;

#[domain_type]
struct PromoCode(String);

number!(PromoBonus, i32);
number!(PromoCapacity, u32);
