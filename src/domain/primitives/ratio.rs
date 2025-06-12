use domain_types_macro::domain_type;
use crate::domain::primitives::validators::ratio_range_validator;

#[domain_type(
    number,
    validated(
        ratio_range_validator,
        error_message("must be between 0 and 1")
    )
)]
pub struct Ratio(f64);
