use std::marker::PhantomData;
use std::ops::Deref;
use derive_where::derive_where;

/// How many rows of `T` there are — the type says what was counted, which a bare number never could.
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

    value: u64,
}

impl <T> Count<T> {
    /// Nothing is ever counted a negative number of times, and the type says so, so there is
    /// nothing here to refuse.
    pub const fn new(value: u64) -> Self {
        Self { _phantom: PhantomData, value }
    }

    /// The same for a value written into the source. It adds no check of its own — there is none
    /// left to make — but it keeps `literal!` working for this type, so a count is written the way
    /// every other domain number is.
    #[allow(dead_code)]
    pub const fn literal(value: u64) -> Self {
        Self::new(value)
    }

    pub const fn value(&self) -> u64 {
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
        let value = i64::try_from(self.value)?;
        <i64 as sqlx::Encode<DB>>::encode_by_ref(&value, buf)
    }
}

impl <'r, T, DB: sqlx::Database> sqlx::Decode<'r, DB> for Count<T>
where i64: sqlx::Decode<'r, DB>
{
    fn decode(value: <DB as sqlx::Database>::ValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let raw = <i64 as sqlx::Decode<DB>>::decode(value)?;
        // `count(*)` is never negative, so this only fires on genuinely corrupted column data.
        Ok(Self::new(u64::try_from(raw)?))
    }
}

// The macro gives every other domain number a `Deref` to its primitive, and this one is written by
// hand, so it is spelled out here to keep them alike.
impl <T> Deref for Count<T> {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl <T> PartialEq<u64> for Count<T> {
    fn eq(&self, other: &u64) -> bool {
        self.value == *other
    }
}

impl <T> PartialOrd<u64> for Count<T> {
    fn partial_cmp(&self, other: &u64) -> Option<std::cmp::Ordering> {
        self.value.partial_cmp(other)
    }
}

impl <T> Default for Count<T> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl <T> From<u64> for Count<T> {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::literal;

    struct Anything;

    #[test]
    fn a_count_is_the_number_it_was_built_from() {
        assert_eq!(Count::<Anything>::new(3), 3);
        assert_eq!(literal!(Count<Anything> = 3), 3);
    }

    #[test]
    fn the_default_count_is_zero() {
        assert_eq!(Count::<Anything>::default(), 0);
    }
}
