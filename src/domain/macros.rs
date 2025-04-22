#[macro_export]
macro_rules! number_wrapper {
    ($name:ident, $value_type:ty) => {
        #[derive(
            Copy, Clone,
            Debug, derive_more::Display,
            PartialEq,
            derive_more::Constructor, derive_more::From,
            derive_more::Add, derive_more::Sub, derive_more::Mul, derive_more::Div, derive_more::Neg,
            derive_more::AddAssign, derive_more::SubAssign, derive_more::MulAssign, derive_more::DivAssign,
            sqlx::Type
        )]
        #[sqlx(transparent)]
        pub struct $name($value_type);

        impl std::ops::Deref for $name {
            type Target = $value_type;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        
        impl $name {
            pub fn value(&self) -> $value_type {
                self.0
            }
        }
    };
}

#[macro_export]
macro_rules! i64_domain {
    ($name:ident) => {
        number_wrapper!($name, i64);
    };
}

#[macro_export]
macro_rules! f64_domain {
    ($name:ident) => {
        number_wrapper!($name, f64);
    };
}
