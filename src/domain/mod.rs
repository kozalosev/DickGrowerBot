pub mod primitives;
pub mod objects;
pub mod enums;

#[macro_export]
macro_rules! pub_use_modules {
    ($name:ident) => {
        mod $name;
        pub use $name::*;
    };
    ($($name:ident),+) => {
        $(pub_use_modules!($name);)+
    }
}
