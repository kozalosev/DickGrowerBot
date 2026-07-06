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
