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

#[macro_export]
macro_rules! signed_number {
    ($name:ident, $inner_type:ty) => {
        #[domain_type(number)]
        struct $name($inner_type);
    };
}

#[macro_export]
macro_rules! positive_number {
    ($name:ident, $inner_type:ty) => {
        #[domain_type(
            number,
            validated(
                greater_or_equal_to_zero,
                error_message("must be greater or equal to zero")
            )
        )]
        struct $name($inner_type);
    }
}
