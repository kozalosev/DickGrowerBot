use std::fmt::{Debug, Display};
use std::ops::{Add, AddAssign, Deref, Div, DivAssign, Mul, MulAssign, Rem, RemAssign, Sub, SubAssign};
use num_traits::{Float, Num, PrimInt};
use crate::errors::DomainAssertionError;

/// Base domain type
pub trait DomainType<T>:
    Clone +
    Debug + Display
where T: 
    Clone +
    Debug + Display
{}

/// Base domain numeric type (ID or number value)
pub trait DomainValue<T>: DomainType<T> +
    Default +
    PartialEq +
    PartialOrd +
    Deref<Target=T>
where T: Num +
    Clone + Default +
    Debug + Display
{
    fn value(&self) -> T;
}

/// Numeric domain type with all arithmetic operations
pub trait DomainNumber<T>: DomainValue<T> + Copy +
    Add + Sub + Mul + Div + Rem +
    Add<T> + Sub<T> + Mul<T> + Div<T> + Rem<T> +
    AddAssign + SubAssign + MulAssign + DivAssign + RemAssign +
    AddAssign<T> + SubAssign<T> + MulAssign<T> + DivAssign<T> + RemAssign<T>
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
    Eq + Ord
where T: PrimInt +
    Clone + Default +
    Debug + Display
{}

/// Integer domain number with all arithmetic operations
pub trait DomainIntegerNumber<T>: DomainNumber<T> + DomainIntegerValue<T>
where T: PrimInt +
    Clone + Default +
    Debug + Display
{}

/// Integer domain number with all arithmetic operations and value validation
pub trait ValidatedDomainIntegerNumber<T>: ValidatedDomainNumber<T> + DomainIntegerValue<T>
where T: PrimInt +
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

/// String domain type
pub trait DomainString: DomainType<String> +
    PartialEq + Eq +
    PartialOrd + Ord +
    AsRef<String> +
    Deref<Target=str>
{
    fn value(&self) -> &str;
}
