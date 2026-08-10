use domain_types_macro::domain_type;

/// What is left to pay back. Never negative — the column carries a `CHECK (debt >= 0)` — so the
/// type says so instead of a validator.
#[domain_type(number)]
struct Debt(u64);
