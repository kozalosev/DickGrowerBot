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
