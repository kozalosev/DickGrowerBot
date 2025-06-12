use domain_types::traits::DomainValue;
use domain_types_macro::domain_type;
use crate::domain::primitives::validators::greater_or_equal_to_zero;
use crate::error;

#[domain_type(number)]
struct Offset(i32);

#[domain_type(
    number,
    validated(
        greater_or_equal_to_zero,
        error_message("must be greater or equal to zero")
    )
)]
struct Limit(i16);

#[domain_type(
    number,
    validated(
        greater_or_equal_to_zero,
        error_message("must be greater or equal to zero")
    )
)]
struct Page(i16);

error!(InvalidPage);

impl From<u16> for Offset {
    fn from(value: u16) -> Self {
        Self(value as i32)
    }
}

impl From<u8> for Limit {
    fn from(value: u8) -> Self {
        Self(value as i16)
    }
}

impl Offset {
    pub fn calculate(page: Page, limit: Limit) -> Offset {
        let value = page.value().checked_add(limit.value())
            .unwrap_or_else(|| page.value().saturating_add(page.value()));
        Self(value as i32)
    }
}

impl Page {
    pub fn first() -> Self {
        Self(0)
    }
}

impl InvalidPage {
    pub fn for_value(value: &str, msg: impl ToString) -> Self {
        Self(format!("{}: {value}", msg.to_string()))
    }
}

#[cfg(test)]
mod test {
    use super::Page;

    #[test]
    fn page_arithmetic() {
        let p0 = Page::first();
        let p1 = p0 + 1;
        let p00 = p1 - 1;
        let p5 = p1 * 5;

        assert_eq!(p0, 0);
        assert_eq!(p1, 1);
        assert_eq!(p00, 0);
        assert_eq!(p5, 5);
    }
}
