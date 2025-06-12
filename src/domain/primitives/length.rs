use domain_types_macro::domain_type;
use super::validators::greater_or_equal_to_zero;

#[domain_type(number)]
struct Length(i64);

#[domain_type(
    number,
    validated(
        greater_or_equal_to_zero,
        error_message("must be greater or equal to zero")
    )
)]
struct PositiveLength(i64);

#[domain_type(number)]
struct LengthChange(i64);

#[domain_type(
    number,
    validated(
        greater_or_equal_to_zero,
        error_message("must be greater or equal to zero")
    )
)]
struct Increment(i32);

#[domain_type(
    number,
    validated(
        greater_or_equal_to_zero,
        error_message("must be greater or equal to zero")
    )
)]
struct Bet(i32);
