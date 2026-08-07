/// An identifier that counts up and never goes below zero — a `serial` column, or an id Telegram
/// guarantees to be positive. Unsigned, so there is nothing to validate, and no `number` flag, so
/// no arithmetic is generated: an id is not a quantity.
#[macro_export]
macro_rules! id {
    ($name:ident) => {
        #[::domain_types_macro::domain_type]
        struct $name(u64);
    };
    ($($name:ident),+) => {
        $(id!($name);)+
    }
}

/// An identifier that may be negative, which for Telegram means a chat: a group's id is negative
/// and a supergroup's begins with -100.
#[macro_export]
macro_rules! signed_id {
    ($name:ident) => {
        #[::domain_types_macro::domain_type]
        struct $name(i64);
    };
    ($($name:ident),+) => {
        $(signed_id!($name);)+
    }
}
