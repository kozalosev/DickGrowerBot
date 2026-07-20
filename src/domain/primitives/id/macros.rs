#[macro_export]
macro_rules! id {
    ($name:ident) => {
        #[::domain_types_macro::domain_type]
        struct $name(i64);
    };
    ($($name:ident),+) => {
        $(id!($name);)+
    }
}

/// Like `id!`, but validated: the inner `i64` must be positive. Unlike `positive_number!`,
/// no `number` flag is set, so no arithmetic operators are generated — an ID is not a number.
#[macro_export]
macro_rules! positive_id {
    ($name:ident) => {
        #[::domain_types_macro::domain_type(
            validated(
                $crate::domain::primitives::validators::i64::positive,
                error_message("must be positive")
            )
        )]
        struct $name(i64);
    };
}
