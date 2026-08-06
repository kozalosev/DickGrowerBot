use std::borrow::Cow;
use std::marker::PhantomData;
use derive_where::derive_where;
use domain_types::errors::DomainAssertionError;
use super::validators;
use crate::literal;

const MUST_BE_NON_NEGATIVE: &str = "Count must be greater or equal to zero";

/// How many rows of `T` there are — the type says what was counted, which a bare `i64` never could.
///
/// `T` is a marker and never a value, which is why the standard traits come from `derive_where`:
/// the plain derives would each demand the same trait of `T`, so a count of something unclonable
/// would stop being clonable itself.
#[derive_where(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[derive(derive_more::Display)]
#[display("Count({value})")]
pub struct Count<T> {
    #[derive_where(skip(Debug))]
    _phantom: PhantomData<T>,

    value: i64,
}

impl <T> Count<T> {
    /// Nothing is ever counted a negative number of times, so a `Count` in hand is always a real
    /// one — validated the way `positive_number!` validates its types, and by the same predicate.
    pub fn new(value: i64) -> Result<Self, DomainAssertionError<i64>> {
        if validators::i64::greater_or_equal_to_zero(&value) {
            Ok(Self::of(value))
        } else {
            Err(DomainAssertionError::new(value, Cow::from(MUST_BE_NON_NEGATIVE)))
        }
    }

    /// The same for a value that is known as the code is written: a negative one fails the build
    /// wherever the call is evaluated as a constant, and panics at once everywhere else.
    pub const fn literal(value: i64) -> Self {
        assert!(validators::i64::greater_or_equal_to_zero(&value), "{}", MUST_BE_NON_NEGATIVE);
        Self::of(value)
    }

    const fn of(value: i64) -> Self {
        Self { _phantom: PhantomData, value }
    }

    pub const fn value(&self) -> i64 {
        self.value
    }
}

// The rest of what `#[domain_type]` would give a wrapper over a primitive. The macro can't be used
// here — it generates for a concrete newtype, not for a generic one — so the useful half is spelled
// out: a count comes straight out of a query, and reads against a plain number.
impl <T, DB: sqlx::Database> sqlx::Type<DB> for Count<T>
where i64: sqlx::Type<DB>
{
    fn type_info() -> DB::TypeInfo {
        <i64 as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        <i64 as sqlx::Type<DB>>::compatible(ty)
    }
}

impl <'q, T, DB: sqlx::Database> sqlx::Encode<'q, DB> for Count<T>
where i64: sqlx::Encode<'q, DB>
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as sqlx::Database>::ArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <i64 as sqlx::Encode<DB>>::encode_by_ref(&self.value, buf)
    }
}

impl <'r, T, DB: sqlx::Database> sqlx::Decode<'r, DB> for Count<T>
where i64: sqlx::Decode<'r, DB>
{
    fn decode(value: <DB as sqlx::Database>::ValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <i64 as sqlx::Decode<DB>>::decode(value)?;
        // `count(*)` is never negative, so this only fires on genuinely corrupted column data.
        Ok(Self::new(raw)?)
    }
}

impl <T> PartialEq<i64> for Count<T> {
    fn eq(&self, other: &i64) -> bool {
        self.value == *other
    }
}

impl <T> PartialOrd<i64> for Count<T> {
    fn partial_cmp(&self, other: &i64) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(other)
    }
}

impl <T> Default for Count<T> {
    fn default() -> Self {
        literal!(Self = 0)
    }
}

impl <T> TryFrom<i64> for Count<T> {
    type Error = DomainAssertionError<i64>;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Anything;

    #[test]
    fn nothing_is_counted_a_negative_number_of_times() {
        assert!(Count::<Anything>::new(-1).is_err());
        assert!(Count::<Anything>::new(0).is_ok());
        assert_eq!(Count::<Anything>::new(3).expect("3 must be a valid count"), 3);
    }

    #[test]
    fn a_valid_literal_is_the_number_it_was_written_as() {
        assert_eq!(literal!(Count<Anything> = 3), 3);
    }

    // A negative literal has no test of its own: `literal!(Count<Anything> = -1)` stops the build,
    // so there is nothing left for a test to observe. That is the point of the macro.

    #[test]
    fn the_default_count_is_zero() {
        assert_eq!(Count::<Anything>::default(), 0);
    }
}
