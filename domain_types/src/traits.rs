use std::fmt::{Debug, Display};
use std::hash::Hash;
use std::ops::{Add, AddAssign, Deref, Mul, MulAssign, Sub, SubAssign};
use num_traits::{Float, Num, PrimInt};
use crate::errors::DomainAssertionError;

/// Base domain type
pub trait DomainType<T>:
    Clone +
    Debug + Display
where T: 
    Clone +
    Debug + Display
{
    fn new(value: T) -> Self;
}

/// Base domain numeric type (ID or number value)
pub trait DomainValue<T>: DomainType<T> +
    Default +
    PartialEq + PartialEq<T> +
    PartialOrd + PartialOrd<T> +
    Deref<Target=T>
where T: Num +
    Clone + Default +
    Debug + Display
{
    fn value(&self) -> T;
}

/// Numeric domain type with all arithmetic operations.
///
/// Division is intentionally not required here: for integer domain numbers the `/` operator
/// (when enabled via the `division_result` macro attribute) produces a *float* domain type,
/// so its `Output` is not `Self` and cannot be expressed as a supertrait bound uniformly.
pub trait DomainNumber<T>: DomainValue<T> + Copy +
    Add + Sub + Mul +
    Add<T> + Sub<T> + Mul<T> +
    AddAssign + SubAssign + MulAssign +
    AddAssign<T> + SubAssign<T> + MulAssign<T>
where T: Num +
    Clone + Default +
    Debug + Display
{}

/// Numeric domain type with all arithmetic operations and value validation
pub trait ValidatedDomainNumber<T>: DomainValue<T> + Copy
where T: Num +
    Clone + Default +
    Debug + Display
{
    fn new(value: T) -> Result<Self, DomainAssertionError<T>>;
}

/// Integer domain type (not a number, i.e., ID or something like that)
pub trait DomainIntegerValue<T>: DomainValue<T> + Copy +
    Eq + Ord + Hash
where T: PrimInt + Hash +
    Clone + Default +
    Debug + Display
{}

/// Integer domain number with all arithmetic operations
pub trait DomainIntegerNumber<T>: DomainNumber<T> + DomainIntegerValue<T>
where T: PrimInt + Hash +
    Clone + Default +
    Debug + Display
{}

/// Integer domain number with all arithmetic operations and value validation
pub trait ValidatedDomainIntegerNumber<T>: ValidatedDomainNumber<T> + DomainIntegerValue<T>
where T: PrimInt + Hash +
    Clone + Default +
    Debug + Display
{}

/// Float domain type (not a number, i.e., ID or something like that)
pub trait DomainFloatValue<T>: DomainValue<T> + Copy
where T: Float +
    Clone + Default +
    Debug + Display
{}

/// Float domain number with all arithmetic operations
pub trait DomainFloatNumber<T>: DomainNumber<T> + DomainFloatValue<T>
where T: Float +
    Clone + Default +
    Debug + Display
{}

/// Float domain number with all arithmetic operations and value validation
pub trait ValidatedDomainFloatNumber<T>: ValidatedDomainNumber<T> + DomainFloatValue<T>
where T: Float +
    Clone + Default +
    Debug + Display
{}

/// A float domain type that may be produced by dividing integer domain numbers.
///
/// Implemented automatically by the `#[domain_type]` macro for float-based domain types:
/// * unvalidated ones construct the value directly (`Output = Self`);
/// * validated ones run their validator (`Output = Result<Self, DomainAssertionError<T>>`),
///   which also catches division by zero (IEEE `inf`/`NaN` fail range validators).
///
/// Integer domain numbers annotated with `division_result(SomeFloatType)` generate `Div` impls
/// whose `Output` is `<SomeFloatType as DivisionResult>::Output`.
pub trait DivisionResult {
    type Output;

    fn from_division(value: f64) -> Self::Output;
}

/// A conversion between number types that can't fail: a value outside the target's range stops at
/// the nearest end of it.
///
/// Deliberately not `From`/`Into`. Those promise the value comes through unchanged, and `.into()`
/// at a call site would say nothing about the clamp, so the loss would happen where nobody named
/// it. Here the name is the warning.
///
/// Integer to integer, and float to integer — the two conversions where a value can be too large to
/// represent at all. A float truncates toward zero once clamped, so a caller who wants rounding
/// says `.round()` first.
pub trait SaturatingInto<T> {
    fn saturating_into(self) -> T;
}

/// A conversion between number types that keeps the magnitude but not every digit: the result is
/// the nearest number the target can represent.
///
/// Integer to float and float to float, where the range is not the problem and the mantissa is.
/// Overflowing an `f32` gives infinity, which is that type's own way of saying "off the end".
pub trait ApproxInto<T> {
    fn approx_into(self) -> T;
}

/// [`SaturatingInto`] between integers. The fallback depends on the *source's* signedness: an
/// unsigned value can only ever be too large, while a signed one has two ends to fall off.
///
/// One shape is generated for every pair, so the widening pairs get a `try_from` whose error arm
/// can't be reached. That is what the `unnecessary_fallible_conversions` allow is for.
macro_rules! impl_saturating_between_integers {
    (unsigned $src:ty => $($dst:ty),+) => {$(
        impl SaturatingInto<$dst> for $src {
            #[allow(clippy::unnecessary_fallible_conversions)]
            fn saturating_into(self) -> $dst {
                <$dst>::try_from(self).unwrap_or(<$dst>::MAX)
            }
        }
    )+};
    (signed $src:ty => $($dst:ty),+) => {$(
        impl SaturatingInto<$dst> for $src {
            #[allow(clippy::unnecessary_fallible_conversions)]
            fn saturating_into(self) -> $dst {
                <$dst>::try_from(self)
                    .unwrap_or(if self < 0 { <$dst>::MIN } else { <$dst>::MAX })
            }
        }
    )+};
}

/// [`SaturatingInto`] from a float to an integer.
///
/// The language already does this: a float-to-integer `as` cast clamps to the target's bounds,
/// truncates toward zero and turns `NaN` into 0 (RFC 2484). Written once here so that no call site
/// has to know the rule.
///
/// Integer to integer is the opposite and gets no shortcut: there `as` wraps, so `300u32` would
/// become `44u8`.
macro_rules! impl_saturating_from_float {
    ($src:ty => $($dst:ty),+) => {$(
        impl SaturatingInto<$dst> for $src {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
            fn saturating_into(self) -> $dst {
                self as $dst
            }
        }
    )+};
}

/// [`ApproxInto`] into a float, from an integer or from the other float.
///
/// Also plain `as`, which already does the right thing here: a target that can't hold every digit
/// of the source takes the nearest value it can, and one that can't hold the magnitude at all takes
/// infinity. Nothing about the conversion changes; it gains a name, so that a call site says which
/// of the three things `as` means is going on.
macro_rules! impl_approx_into_float {
    ($src:ty => $($dst:ty),+) => {$(
        impl ApproxInto<$dst> for $src {
            #[allow(clippy::cast_lossless, clippy::cast_precision_loss, clippy::cast_possible_truncation)]
            fn approx_into(self) -> $dst {
                self as $dst
            }
        }
    )+};
}

// Only the pairs that can lose something: every narrowing, every signed-to-unsigned, and
// `usize`/`isize` as a source, neither having a portable `From` to a fixed width. What is missing
// is what std converts exactly — `the_pairs_left_out_are_the_ones_std_converts` names each one and
// stops compiling if it was not exact after all.
impl_saturating_between_integers!(unsigned u8 => i8);
impl_saturating_between_integers!(unsigned u16 => i8, i16, u8);
impl_saturating_between_integers!(unsigned u32 => i8, i16, i32, isize, u8, u16, usize);
impl_saturating_between_integers!(unsigned u64 => i8, i16, i32, i64, isize, u8, u16, u32, usize);
impl_saturating_between_integers!(unsigned usize => i8, i16, i32, i64, isize, u8, u16, u32, u64);
impl_saturating_between_integers!(signed i8 => u8, u16, u32, u64, usize);
impl_saturating_between_integers!(signed i16 => i8, u8, u16, u32, u64, usize);
impl_saturating_between_integers!(signed i32 => i8, i16, isize, u8, u16, u32, u64, usize);
impl_saturating_between_integers!(signed i64 => i8, i16, i32, isize, u8, u16, u32, u64, usize);
impl_saturating_between_integers!(signed isize => i8, i16, i32, i64, u8, u16, u32, u64, usize);

impl_saturating_from_float!(f32 => i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
impl_saturating_from_float!(f64 => i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

// Only the pairs that can actually lose something. Everything absent here is exact — std has a
// `From` for it — and the exact case must say `From`, the same rule `cast_lossless` enforces for
// `as`. An `f64` holds every `i32` and every `u32`; an `f32` holds neither, having 24 bits of
// mantissa against 53.
impl_approx_into_float!(i32 => f32);
impl_approx_into_float!(u32 => f32);
impl_approx_into_float!(i64 => f32, f64);
impl_approx_into_float!(u64 => f32, f64);
impl_approx_into_float!(isize => f32, f64);
impl_approx_into_float!(usize => f32, f64);
impl_approx_into_float!(f64 => f32);

/// String domain type
pub trait DomainString: DomainType<String> +
    PartialEq + Eq +
    PartialOrd + Ord +
    Hash +
    AsRef<String> +
    Deref<Target=str>
{
    fn value(&self) -> &str;
}

#[cfg(test)]
mod test {
    use super::{ApproxInto, SaturatingInto};

    /// Every integer pair `SaturatingInto` leaves out, converted with the `From` that is the
    /// reason it was left out. A pair listed here that std cannot convert stops the build — which
    /// is what keeps the two halves one table rather than two opinions about it.
    #[test]
    fn the_pairs_left_out_are_the_ones_std_converts() {
        let _ = (u16::from(1u8), u32::from(1u8), u64::from(1u8), usize::from(1u8),
                 i16::from(1u8), i32::from(1u8), i64::from(1u8), isize::from(1u8));
        let _ = (u32::from(1u16), u64::from(1u16), usize::from(1u16),
                 i32::from(1u16), i64::from(1u16));
        let _ = (u64::from(1u32), i64::from(1u32));
        let _ = (i16::from(1i8), i32::from(1i8), i64::from(1i8), isize::from(1i8));
        let _ = (i32::from(1i16), i64::from(1i16), isize::from(1i16));
        let _ = i64::from(1i32);
    }

    #[test]
    fn a_value_the_target_can_hold_comes_through_untouched() {
        let value: i64 = 1234u64.saturating_into();
        assert_eq!(value, 1234);
    }

    #[test]
    fn an_unsigned_value_too_large_stops_at_the_top() {
        let value: i64 = u64::MAX.saturating_into();
        assert_eq!(value, i64::MAX);
    }

    /// The reason this is a trait and not a generated `From`: a fallback of "the maximum" would
    /// turn a value that was slightly too small into the largest one there is.
    #[test]
    fn a_negative_value_stops_at_the_bottom_rather_than_the_top() {
        let value: u32 = (-5i64).saturating_into();
        assert_eq!(value, 0);
    }

    #[test]
    fn a_signed_value_too_large_still_stops_at_the_top() {
        let value: u8 = 300i32.saturating_into();
        assert_eq!(value, u8::MAX);
    }

    #[test]
    fn a_float_is_clamped_and_truncated_toward_zero() {
        let rounded_down: i32 = 7.9f64.saturating_into();
        assert_eq!(rounded_down, 7);

        let too_large: i32 = 1e30f64.saturating_into();
        assert_eq!(too_large, i32::MAX);

        let too_small: i32 = (-1e30f64).saturating_into();
        assert_eq!(too_small, i32::MIN);
    }

    #[test]
    fn a_float_that_is_not_a_number_becomes_zero() {
        let value: i64 = f64::NAN.saturating_into();
        assert_eq!(value, 0);
    }

    #[test]
    fn a_number_a_float_can_hold_exactly_stays_exact() {
        let value: f64 = 1234u64.approx_into();
        assert_eq!(value, 1234.0);
    }

    #[test]
    fn a_number_with_more_digits_than_the_float_has_takes_the_nearest() {
        let value: f64 = u64::MAX.approx_into();
        assert_eq!(value, 18446744073709551616.0);
    }

    #[test]
    fn a_magnitude_the_float_cannot_reach_becomes_infinity() {
        let value: f32 = f64::MAX.approx_into();
        assert!(value.is_infinite());
        assert!(value.is_sign_positive());
    }
}
