//! Integration tests for the `#[domain_type]` attribute macro.
//!
//! All types here use `features(not_database_type)` so that the tests
//! don't need sqlx as a dev-dependency; the sqlx integration is exercised
//! by the main crate itself.

use std::str::FromStr;
use domain_types_macro::domain_type;

const fn ge_zero(value: &i32) -> bool {
    *value >= 0
}

const fn ratio_range(value: &f64) -> bool {
    *value >= 0.0 && *value <= 1.0
}

#[domain_type(features(not_database_type))]
struct Id(i64);

#[domain_type(number, features(not_database_type))]
struct Count(i32);

#[domain_type(number, features(not_database_type))]
struct Speed(f64);

#[domain_type(
    number,
    validated(ge_zero, error_message("must be greater or equal to zero")),
    features(not_database_type)
)]
struct Positive(i32);

#[domain_type(
    number,
    validated(ratio_range, error_message("must be between 0 and 1")),
    features(not_database_type)
)]
struct Ratio(f64);

#[domain_type(number, division_result(Ratio), features(not_database_type))]
struct Wins(i64);

#[domain_type(number, division_result(Speed), features(not_database_type))]
struct Meters(i64);

#[domain_type(features(not_database_type))]
struct Login(String);

mod value_basics {
    use super::*;

    #[test]
    fn new_value_deref_display() {
        let id = Id::new(42);
        assert_eq!(*id, 42);
        assert_eq!(id.to_string(), "42");
        assert_eq!(id, 42i64);
        assert!(id > 41i64);
    }

    #[test]
    fn from_str() {
        let id = Id::from_str("17").unwrap();
        assert_eq!(id, 17i64);
        assert!(Id::from_str("not a number").is_err());
    }

    #[test]
    fn is_zero() {
        assert!(Id::new(0).is_zero());
        assert!(!Id::new(1).is_zero());
        assert!(Count::new(0).is_zero());
        assert!(Speed::new(0.0).is_zero());
    }
}

mod unvalidated_integer_arithmetic {
    use super::*;

    #[test]
    fn operators_are_infallible() {
        let c = Count::new(2);
        assert_eq!(c + Count::new(3), 5);
        assert_eq!(c - 5, -3);
        assert_eq!(c * 10, 20);
        assert_eq!(-c, -2);

        let mut acc = Count::new(1);
        acc += 4;
        acc *= Count::new(2);
        acc -= 3;
        assert_eq!(acc, 7);
    }

    #[test]
    fn operators_saturate_on_overflow() {
        assert_eq!(Count::new(i32::MAX) + 1, i32::MAX);
        assert_eq!(Count::new(i32::MIN) - 1, i32::MIN);
        assert_eq!(Count::new(i32::MAX) * 2, i32::MAX);
    }

    #[test]
    fn explicit_overflowing_methods_report_overflow() {
        let (wrapped, overflowed) = Count::new(i32::MAX).overflowing_add_primitive(1);
        assert!(overflowed);
        assert_eq!(wrapped, i32::MIN);

        let (ok, overflowed) = Count::new(10).overflowing_mul(Count::new(3));
        assert!(!overflowed);
        assert_eq!(ok, 30);
    }

    #[test]
    fn integer_division_methods_return_self() {
        assert_eq!(Count::new(7).saturating_div(Count::new(2)), 3);
        let (rem, overflowed) = Count::new(7).overflowing_rem_primitive(4);
        assert!(!overflowed);
        assert_eq!(rem, 3);
    }
}

mod validated_integer_arithmetic {
    use super::*;

    #[test]
    fn constructor_validates() {
        assert!(Positive::new(5).is_ok());
        assert!(Positive::new(0).is_ok());
        assert!(Positive::new(-1).is_err());
    }

    #[test]
    fn literal_works_in_const_context() {
        const THREE: Positive = Positive::literal(3);
        assert_eq!(THREE, 3);
    }

    #[test]
    fn operators_return_results() {
        let p = Positive::literal(2);
        assert_eq!((p + Positive::literal(3)).unwrap(), 5);
        assert_eq!((p * 4).unwrap(), 8);
        // Subtraction below zero violates the invariant
        assert!((p - 5).is_err());
    }

    #[test]
    fn overflow_becomes_an_error() {
        let max = Positive::literal(i32::MAX);
        assert!(max.overflowing_add_primitive(1).is_err());
        // The saturating flavor clamps to a still-valid value
        assert_eq!(max.saturating_add_primitive(1).unwrap(), i32::MAX);
    }
}

mod float_types {
    use super::*;

    #[test]
    fn validated_float_constructor_and_ops() {
        assert!(Ratio::new(0.5).is_ok());
        assert!(Ratio::new(1.5).is_err());
        assert!(Ratio::new(-0.1).is_err());

        let half = Ratio::literal(0.5);
        assert_eq!((half + 0.25).unwrap(), 0.75);
        assert!((half + half).unwrap() == 1.0);
        assert!((half + 0.75).is_err());
    }

    #[test]
    fn unvalidated_float_ops() {
        let s = Speed::new(2.5);
        assert_eq!(s + Speed::new(1.5), 4.0);
        assert_eq!(s / 2.0, 1.25);
    }
}

mod division_result {
    use super::*;

    #[test]
    fn division_produces_validated_float_domain_type() {
        let ratio = (Wins::new(1) / Wins::new(2)).unwrap();
        assert_eq!(ratio, 0.5);

        let ratio = (Wins::new(3) / 4i64).unwrap();
        assert_eq!(ratio, 0.75);
    }

    #[test]
    fn division_by_zero_fails_validation() {
        assert!((Wins::new(1) / Wins::new(0)).is_err());
    }

    #[test]
    fn out_of_range_quotient_fails_validation() {
        assert!((Wins::new(5) / Wins::new(2)).is_err());
    }

    #[test]
    fn division_produces_unvalidated_float_domain_type() {
        let speed: Speed = Meters::new(10) / Meters::new(4);
        assert_eq!(speed, 2.5);
    }
}

mod string_types {
    use super::*;

    #[test]
    fn construction_and_access() {
        let login = Login::new("kozalo".to_owned());
        assert_eq!(&*login, "kozalo");
        assert_eq!(login.to_string(), "kozalo");

        let login = Login::of(42);
        assert_eq!(&*login, "42");
    }

    #[test]
    fn equality_and_ordering() {
        let a = Login::of("abc");
        let b = Login::new("abc".to_owned());
        assert_eq!(a, b);
        assert!(a < Login::of("abd"));
    }
}
