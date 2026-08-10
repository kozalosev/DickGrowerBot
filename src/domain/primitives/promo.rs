use domain_types_macro::domain_type;
use crate::domain::primitives::validators::promo_code_validator;
use crate::number;

#[domain_type(
    validated(
        promo_code_validator,
        error_message("must be 4 to 16 letters, digits, underscores or hyphens"),
    ),
)]
struct PromoCode(String);

number!(PromoBonus, i32);
number!(PromoCapacity, u32);
