use domain_types_macro::domain_type;

const fn ratio_range_validator(x: &f64) -> bool {
    *x >= 0.0 && *x <= 1.0
}

#[domain_type(
    number,
    validated(
        ratio_range_validator,
        error_message("must be between 0 and 1")
    )
)]
struct Ratio(f64);

#[domain_type]
struct Id(i64);

#[domain_type]
struct Username(String);

// #[domain_type]
// struct Bad {}

fn main() {
    let invalid_ratio = Ratio::literal(1.0);

    // 1.0.

    println!("invalid_ratio = {invalid_ratio}");
}
