use domain_types_macro::domain_type;
use super::::error;

fn foo() {
    greater_or_equal_to_zero()
}

#[domain_type(
    number,
    validated(
        greater_or_equal_to_zero,
        error_message("must be greater or equal to zero")
    )
)]
struct Page(i32);

error!(InvalidPage);

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
