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
    use super::{FloatPercentage, Percentage, Ratio};

    #[test]
    fn percentage_display() {
        assert_eq!(Percentage::literal(0).to_string(), "0%");
        assert_eq!(Percentage::literal(33).to_string(), "33%");
        assert_eq!(Percentage::literal(100).to_string(), "100%");
    }

    #[test]
    fn float_percentage_display() {
        assert_eq!(FloatPercentage::literal(0.0).to_string(), "0.00%");
        assert_eq!(FloatPercentage::literal(10.0).to_string(), "10.00%");
        assert_eq!(FloatPercentage::literal(33.333).to_string(), "33.33%");
    }

    #[test]
    fn ratio_scale() {
        assert_eq!(Ratio::literal(0.1).scale(50.0), 5.0);
        assert_eq!(Ratio::literal(0.0).scale(50.0), 0.0);
        assert_eq!(Ratio::literal(1.0).scale(50.0), 50.0);
    }

    #[test]
    fn ratio_percentage_conversions() {
        let ratio = Ratio::literal(0.1);
        assert_eq!(Percentage::from(ratio), Percentage::literal(10));
        assert_eq!(FloatPercentage::from(ratio), FloatPercentage::literal(10.0));
    }

    #[test]
    fn ratio_percentage_lower_bound() {
        assert_eq!(Percentage::from(Ratio::literal(0.0)), Percentage::literal(0));
        assert_eq!(FloatPercentage::from(Ratio::literal(0.0)), FloatPercentage::literal(0.0));
    }

    #[test]
    fn ratio_percentage_upper_bound() {
        assert_eq!(Percentage::from(Ratio::literal(1.0)), Percentage::literal(100));
        assert_eq!(FloatPercentage::from(Ratio::literal(1.0)), FloatPercentage::literal(100.0));
    }

    #[test]
    fn ratio_percentage_rounds_half_away_from_zero() {
        // 0.125 and 100 are both exactly representable in f64, so the product (12.5) is exact
        // too - no floating-point drift to worry about in this assertion.
        assert_eq!(Percentage::from(Ratio::literal(0.125)), Percentage::literal(13));
    }

    #[test]
    fn ratio_percentage_rounds_toward_each_bound_without_panicking() {
        // Values close enough to 0/1 that rounding lands exactly on the boundary - this is
        // what the old clamping `match` in `percentage()` used to guard against; make sure
        // dropping it didn't reintroduce a panic at the edges.
        assert_eq!(Percentage::from(Ratio::literal(0.001)), Percentage::literal(0));
        assert_eq!(Percentage::from(Ratio::literal(0.999)), Percentage::literal(100));
    }
}
