//! Integration tests for the `#[domain_type]` attribute macro.
//!
//! Every type here ends up `sqlx`-embeddable one way or another: signed/float/string inner types
//! `#[sqlx(transparent)]`-derive directly, and unsigned inner types (`u8`/`u16`/`u32`) get hand-written
//! impls bumped to the smallest signed integer that always fits (see `SqlxMode` in `src/lib.rs`) —
//! which is why `sqlx` is a real dev-dependency here now, unlike when these tests opted out via the
//! since-removed `not_database_type` flag.

use std::str::FromStr;
use domain_types_macro::domain_type;

const fn ge_zero(value: &i32) -> bool {
    *value >= 0
}

const fn ge_zero_i64(value: &i64) -> bool {
    *value >= 0
}

const fn ratio_range(value: &f64) -> bool {
    *value >= 0.0 && *value <= 1.0
}

#[domain_type]
struct Id(i64);

#[domain_type(number)]
struct Count(i32);

#[domain_type(number)]
struct Speed(f64);

#[domain_type(
    number,
    validated(ge_zero, error_message("must be greater or equal to zero")),
)]
struct Positive(i32);

#[domain_type(
    number,
    validated(ratio_range, error_message("must be between 0 and 1")),
)]
struct Ratio(f64);

#[domain_type(number, division_result(Ratio))]
struct Wins(i64);

// A validated ID/value type: no `number`, so no arithmetic surface is generated,
// unlike `Positive` above.
#[domain_type(
    validated(ge_zero_i64, error_message("must be greater or equal to zero")),
)]
struct PositiveId(i64);

// Same idea, over a float: a validated value type with no arithmetic surface.
#[domain_type(
    validated(ratio_range, error_message("must be between 0 and 1")),
)]
struct RatioValue(f64);

#[domain_type(number, division_result(Speed))]
struct Meters(i64);

#[domain_type]
struct Login(String);

// `no_auto_display` types must provide Display manually (DomainType requires it).
#[domain_type(features(no_auto_display))]
struct Opaque(i64);

impl std::fmt::Display for Opaque {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "opaque[{}]", self.0)
    }
}

// Unsigned inner types: `SqlxMode::Bumped` for u8/u16/u32 (bumped to i16/i32/i64 respectively —
// see `sqlx_embeddable_for_every_bump_width` below), `SqlxMode::None` for u64 (no signed Postgres
// type wide enough to bump into, so no `sqlx` impls at all — it just can't bind into a query).
#[domain_type(number)]
struct TinyCount(u8);

#[domain_type(number)]
struct MediumCount(u16);

#[domain_type(number)]
struct LargeCount(u32);

#[domain_type(number)]
struct HugeCount(u64);

mod value_basics {
    use super::*;

    #[test]
    fn new_value_deref_display() {
        let id = Id::new(42);
        assert_eq!(*id, 42);
        assert_eq!(id.to_string(), "42");
        assert_eq!(id, 42i64);
        assert!(id > 41);
    }

    #[test]
    fn from_str() {
        let id = Id::from_str("17").unwrap();
        assert_eq!(id, 17);
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

    #[test]
    fn from_str_validates() {
        assert!(Positive::from_str("5").is_ok());
        assert!(Positive::from_str("-1").is_err());
        assert!(Positive::from_str("not a number").is_err());
    }
}

mod validated_value_types {
    use super::*;

    #[test]
    fn constructor_validates() {
        assert!(PositiveId::new(5).is_ok());
        assert!(PositiveId::new(0).is_ok());
        assert!(PositiveId::new(-1).is_err());
    }

    #[test]
    fn literal_works_in_const_context() {
        const ID: PositiveId = PositiveId::literal(7);
        assert_eq!(ID, 7);
    }

    #[test]
    fn eq_ord_hash_usable_without_arithmetic() {
        use std::collections::HashSet;

        let a = PositiveId::literal(1);
        let b = PositiveId::literal(2);
        assert!(a < b);
        assert_ne!(a, b);

        let mut set = HashSet::new();
        set.insert(a);
        set.insert(PositiveId::literal(1));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn from_str_validates() {
        let id = PositiveId::from_str("42").unwrap();
        assert_eq!(id, 42);
        assert!(PositiveId::from_str("-1").is_err());
        assert!(PositiveId::from_str("not a number").is_err());
    }

    // A validated float without `number`: proves `validated` alone (no `number`) validates
    // for float inner types too, not just integers.
    #[test]
    fn float_constructor_validates() {
        assert!(RatioValue::new(0.5).is_ok());
        assert!(RatioValue::new(1.5).is_err());
        assert!(RatioValue::new(-0.1).is_err());
    }

    #[test]
    fn float_from_str_validates() {
        assert!(RatioValue::from_str("0.5").is_ok());
        assert!(RatioValue::from_str("1.5").is_err());
        assert!(RatioValue::from_str("not a number").is_err());
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
        assert_eq!((half + half).unwrap(), 1.0);
        assert!((half + 0.75).is_err());
    }

    #[test]
    fn from_str_validates() {
        assert!(Ratio::from_str("0.5").is_ok());
        assert!(Ratio::from_str("1.5").is_err());
        assert!(Ratio::from_str("not a number").is_err());
    }

    #[test]
    fn unvalidated_float_ops() {
        let s = Speed::new(2.5);
        assert_eq!(s + Speed::new(1.5), 4.0);
        assert_eq!(s / 2.0, 1.25);
    }
}

/// The two conversions that lose something. Each generated impl only forwards, so what is proven
/// here is that it reaches the primitive impl — the clamping itself is tested in `domain_types`.
mod lossy_conversions {
    use domain_types::traits::{ApproxInto, SaturatingInto};
    use super::*;

    #[test]
    fn a_value_the_target_can_hold_comes_through_untouched() {
        let value: i64 = HugeCount::new(1234).saturating_into();
        assert_eq!(value, 1234);
    }

    #[test]
    fn a_value_too_large_for_the_target_stops_at_its_top() {
        let value: i64 = HugeCount::new(u64::MAX).saturating_into();
        assert_eq!(value, i64::MAX);
    }

    #[test]
    fn a_negative_value_stops_at_the_bottom_of_an_unsigned_target() {
        let value: u32 = Id::new(-5).saturating_into();
        assert_eq!(value, 0);
    }

    /// An id-like type is not a `number` and has no arithmetic, but it still crosses into foreign
    /// APIs, so it converts like the rest.
    #[test]
    fn an_id_like_type_converts_too() {
        let value: i32 = Id::new(7).saturating_into();
        assert_eq!(value, 7);
    }

    #[test]
    fn an_integer_type_reaches_a_float() {
        let value: f64 = Count::new(3).approx_into();
        assert_eq!(value, 3.0);
    }

    #[test]
    fn a_float_type_narrows_to_the_smaller_float() {
        let value: f32 = Speed::new(2.5).approx_into();
        assert_eq!(value, 2.5);
    }

    #[test]
    fn a_float_type_reaches_an_integer_by_truncating() {
        let value: i64 = Speed::new(7.9).saturating_into();
        assert_eq!(value, 7);
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

mod combined_features {
    use super::*;

    #[test]
    fn no_auto_display_types_use_the_manual_impl() {
        assert_eq!(Opaque::new(1).value(), 1);
        assert_eq!(Opaque::new(1).to_string(), "opaque[1]");
    }
}

mod bumped_unsigned_types {
    use super::*;

    #[test]
    fn value_basics_hold_for_every_width() {
        assert_eq!(TinyCount::new(200), 200);
        assert_eq!(MediumCount::new(60_000), 60_000);
        assert_eq!(LargeCount::new(4_000_000_000), 4_000_000_000);
        assert_eq!(HugeCount::new(10_000_000_000_000_000_000), 10_000_000_000_000_000_000);
    }

    // Compile-time-only proof (no live DB needed) that u8/u16/u32-backed types are directly
    // bindable into a Postgres query via sqlx, each bumped to the smallest signed integer that
    // always fits (i16/i32/i64 respectively).
    fn assert_sqlx_postgres_embeddable<T>()
    where
        T: for<'q> sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
    {
    }

    #[test]
    fn sqlx_embeddable_for_every_bump_width() {
        assert_sqlx_postgres_embeddable::<TinyCount>();
        assert_sqlx_postgres_embeddable::<MediumCount>();
        assert_sqlx_postgres_embeddable::<LargeCount>();
        // No matching call for HugeCount (u64) — see the comment above its definition.
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
