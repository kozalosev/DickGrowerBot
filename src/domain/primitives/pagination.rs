use domain_types::traits::DomainValue;
use domain_types_macro::domain_type;
use crate::domain::primitives::validators::greater_or_equal_to_zero;
use crate::{error, positive_number, signed_number};

signed_number!(Offset, i32);
positive_number!(Limit, i16);
positive_number!(Page, i16);

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
        let p1 = (p0 + 1).unwrap();
        let p00 = (p1 - 1).unwrap();
        let p5 = (p1 * 5).unwrap();

        assert_eq!(p0, Page::literal(0));
        assert_eq!(p1, Page::literal(1));
        assert_eq!(p00, Page::literal(0));
        assert_eq!(p5, Page::literal(5));
    }
}
