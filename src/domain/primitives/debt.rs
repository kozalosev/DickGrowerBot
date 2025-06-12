use domain_types_macro::domain_type;

#[domain_type(number)]
struct Debt(i64);

impl From<u32> for Debt {
    fn from(value: u32) -> Self {
        Self(value.into())
    }
}
