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
