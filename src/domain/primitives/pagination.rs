use domain_types_macro::domain_type;
use crate::{error, number};

number!(Offset, i32);
number!(Limit, u16);
number!(Page, u16);

error!(InvalidPage);

impl From<u16> for Offset {
    fn from(value: u16) -> Self {
        Self(value as i32)
    }
}

impl From<u8> for Limit {
    fn from(value: u8) -> Self {
        Self(value as u16)
    }
}

impl Offset {
    /// Where the page starts. Two `u16` can multiply out to more than an `i32` holds, so the
    /// product is taken in `i64` and cut down to the offset a query can carry.
    pub fn calculate(page: Page, limit: Limit) -> Offset {
        let value = (page.value() as i64) * (limit.value() as i64);
        Self(value.min(i32::MAX as i64) as i32)
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
    use crate::literal;
    use super::{Limit, Offset, Page};

    #[test]
    fn page_arithmetic() {
        let p0 = Page::first();
        let p1 = p0 + 1;
        let p00 = p1 - 1;
        let p5 = p1 * 5;

        assert_eq!(p0, literal!(Page = 0));
        assert_eq!(p1, literal!(Page = 1));
        assert_eq!(p00, literal!(Page = 0));
        assert_eq!(p5, literal!(Page = 5));
    }

    /// A page below the first one saturates instead of wrapping around to 65535.
    #[test]
    fn the_page_before_the_first_one_is_the_first_one() {
        assert_eq!(Page::first() - 1, Page::first());
    }

    #[test]
    fn the_offset_is_the_page_times_the_limit() {
        let offset = Offset::calculate(literal!(Page = 3), literal!(Limit = 10));
        assert_eq!(offset, literal!(Offset = 30));
    }

    /// The product of two `u16` reaches 4.29 billion, well past what an `i32` offset can hold, so
    /// it is cut down rather than allowed to wrap.
    #[test]
    fn an_offset_too_large_to_hold_is_cut_down() {
        let offset = Offset::calculate(literal!(Page = 65535), literal!(Limit = 65535));
        assert_eq!(offset, literal!(Offset = i32::MAX));
    }
}
