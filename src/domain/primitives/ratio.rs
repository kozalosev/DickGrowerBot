use domain_types_macro::domain_type;
use crate::domain::primitives::validators::{ratio_range_validator, percentage_range_validator, percentage_range_validator_f64};

#[domain_type(
    number,
    validated(
        ratio_range_validator,
        error_message("must be between 0 and 1")
    )
)]
pub struct Ratio(f64);

impl Ratio {
    /// Scales an arbitrary magnitude by this ratio, e.g. a coefficient applied to a length.
    /// The result is a plain `f64`, not another `Ratio`: unlike this type's `Mul<f64>` (which
    /// validates the product back into 0.0..=1.0), `magnitude` isn't bounded to that range, so
    /// the product generally isn't a valid `Ratio` either.
    pub fn scale(self, magnitude: f64) -> f64 {
        self.value() * magnitude
    }
}

#[domain_type(
    number,
    validated(
        percentage_range_validator,
        error_message("must be between 0 and 100")
    ),
    features(no_auto_display)
)]
pub struct Percentage(i32);

impl std::fmt::Display for Percentage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}%", self.value())
    }
}

#[domain_type(
    number,
    validated(
        percentage_range_validator_f64,
        error_message("must be between 0.0 and 100.0")
    ),
    features(no_auto_display)
)]
pub struct FloatPercentage(f64);

impl std::fmt::Display for FloatPercentage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.2}%", self.value())
    }
}

impl From<Ratio> for Percentage {
    fn from(ratio: Ratio) -> Self {
        // Ratio is validated to 0.0..=1.0, so the product is always within 0.0..=100.0,
        // and rounding a value already in that range can't leave it either.
        Percentage::new((ratio.value() * 100.0).round() as i32)
            .expect("a correct ratio must be convertible to percentage")
    }
}

impl From<Ratio> for FloatPercentage {
    fn from(ratio: Ratio) -> Self {
        FloatPercentage::new(ratio.value() * 100.0)
            .expect("a correct ratio must be convertible to a float percentage")
    }
}

#[cfg(test)]
mod test {
    use crate::literal;
    use super::{FloatPercentage, Percentage, Ratio};

    #[test]
    fn percentage_display() {
        assert_eq!(literal!(Percentage = 0).to_string(), "0%");
        assert_eq!(literal!(Percentage = 33).to_string(), "33%");
        assert_eq!(literal!(Percentage = 100).to_string(), "100%");
    }

    #[test]
    fn float_percentage_display() {
        assert_eq!(literal!(FloatPercentage = 0.0).to_string(), "0.00%");
        assert_eq!(literal!(FloatPercentage = 10.0).to_string(), "10.00%");
        assert_eq!(literal!(FloatPercentage = 33.333).to_string(), "33.33%");
    }

    #[test]
    fn ratio_scale() {
        assert_eq!(literal!(Ratio = 0.1).scale(50.0), 5.0);
        assert_eq!(literal!(Ratio = 0.0).scale(50.0), 0.0);
        assert_eq!(literal!(Ratio = 1.0).scale(50.0), 50.0);
    }

    #[test]
    fn ratio_percentage_conversions() {
        let ratio = literal!(Ratio = 0.1);
        assert_eq!(Percentage::from(ratio), literal!(Percentage = 10));
        assert_eq!(FloatPercentage::from(ratio), literal!(FloatPercentage = 10.0));
    }

    #[test]
    fn ratio_percentage_lower_bound() {
        assert_eq!(Percentage::from(literal!(Ratio = 0.0)), literal!(Percentage = 0));
        assert_eq!(FloatPercentage::from(literal!(Ratio = 0.0)), literal!(FloatPercentage = 0.0));
    }

    #[test]
    fn ratio_percentage_upper_bound() {
        assert_eq!(Percentage::from(literal!(Ratio = 1.0)), literal!(Percentage = 100));
        assert_eq!(FloatPercentage::from(literal!(Ratio = 1.0)), literal!(FloatPercentage = 100.0));
    }

    #[test]
    fn ratio_percentage_rounds_half_away_from_zero() {
        // 0.125 and 100 are both exactly representable in f64, so the product (12.5) is exact
        // too - no floating-point drift to worry about in this assertion.
        assert_eq!(Percentage::from(literal!(Ratio = 0.125)), literal!(Percentage = 13));
    }

    #[test]
    fn ratio_percentage_rounds_toward_each_bound_without_panicking() {
        // Values close enough to 0/1 that rounding lands exactly on the boundary - this is
        // what the old clamping `match` in `percentage()` used to guard against; make sure
        // dropping it didn't reintroduce a panic at the edges.
        assert_eq!(Percentage::from(literal!(Ratio = 0.001)), literal!(Percentage = 0));
        assert_eq!(Percentage::from(literal!(Ratio = 0.999)), literal!(Percentage = 100));
    }
}
