#[macro_export]
macro_rules! error {
    ($name:ident) => {
        #[derive(Debug, derive_more::Error, derive_more::Display)]
        pub struct $name(#[error(not(source))] String);
        
        impl $name {
            pub fn message(msg: impl ToString) -> Self {
                Self(msg.to_string())
            }
        }
    };
}

/// A domain number: a wrapper over an integer, with the arithmetic that goes with it.
///
/// A quantity that can never be negative takes an unsigned inner type, which says so in the type
/// system. Postgres has no unsigned column, so the macro stores such a type in the signed integer
/// of the same width.
#[macro_export]
macro_rules! number {
    ($name:ident, $inner_type:ty) => {
        #[domain_type(number)]
        struct $name($inner_type);
    };
}
