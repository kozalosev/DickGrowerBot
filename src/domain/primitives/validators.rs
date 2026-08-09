//! Validators must be `const fn` so that the macro-generated `Type::literal(...)` constructors can
//! evaluate them while the code is compiled.

pub const fn ratio_range_validator(x: &f64) -> bool {
    *x >= 0.0 && *x <= 1.0
}

pub const fn percentage_range_validator(x: &i32) -> bool {
    0 <= *x && *x <= 100
}

pub const fn percentage_range_validator_f64(x: &f64) -> bool {
    *x >= 0.0 && *x <= 100.0
}
