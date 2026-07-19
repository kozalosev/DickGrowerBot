//! Validators must be `const fn` so that the macro-generated `Type::literal(...)`
//! constructors can evaluate them at compile time. Generic `const fn`s cannot call
//! trait methods (like `PartialOrd::ge`) on stable Rust, so a concrete function is
//! stamped per primitive type instead, inside a module named after that type
//! (the same way `std::i16` coexists with the primitive `i16`). This lets
//! `positive_number!` build the validator path from its type argument directly:
//! `validators::$inner_type::greater_or_equal_to_zero`.

macro_rules! ge_zero_validators {
    ($($ty:ident),+ $(,)?) => {
        $(
            pub mod $ty {
                pub const fn greater_or_equal_to_zero(value: &$ty) -> bool {
                    *value >= 0
                }

                // Not every type has a current consumer (only `i64`, via `positive_id!`);
                // kept alongside its sibling instead of hand-duplicating the module per type.
                #[allow(dead_code)]
                pub const fn positive(value: &$ty) -> bool {
                    *value > 0
                }
            }
        )+
    };
}

ge_zero_validators!(i16, i32, i64);

pub const fn ratio_range_validator(x: &f64) -> bool {
    *x >= 0.0 && *x <= 1.0
}

pub const fn percentage_range_validator(x: &i32) -> bool {
    0 <= *x && *x <= 100
}

pub const fn percentage_range_validator_f64(x: &f64) -> bool {
    *x >= 0.0 && *x <= 100.0
}
