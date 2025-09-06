use domain_types_macro::domain_type;
use crate::domain::primitives::validators::{ratio_range_validator, percentage_range_validator};

#[domain_type(
    number,
    validated(
        ratio_range_validator,
        error_message("must be between 0 and 1")
    )
)]
pub struct Ratio(f64);

#[domain_type(
    number,
    validated(
        percentage_range_validator,
        error_message("must be between 0 and 100")
    )
)]
pub struct Percentage(i32);

impl Ratio {
    pub fn percentage(self) -> Percentage {
        let value = match (self.value() * 100.0).round() as i32 {
            ..=0 => 0,
            100.. => 100,
            x => x
        };
        Percentage::new(value)
            .expect("a correct ratio must be convertible to percentage")
    }
}
