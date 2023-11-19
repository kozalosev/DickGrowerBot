use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::ops::{Add, Mul, Sub};

#[derive(Copy, Clone, Debug)]
pub struct Page(pub u32);

#[derive(Debug)]
pub struct InvalidPage(String);

impl Page {
    pub fn first() -> Self {
        Self(0)
    }

    pub fn is_first(&self) -> bool {
        self.0 == 0
    }
}

impl Display for Page {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.to_string().as_str())
    }
}

impl Sub<u32> for Page {
    type Output = Self;

    fn sub(self, rhs: u32) -> Self::Output {
        Self(self.0 - rhs)
    }
}

impl Add<u32> for Page {
    type Output = Self;

    fn add(self, rhs: u32) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl Mul<u32> for Page {
    type Output = u32;

    fn mul(self, rhs: u32) -> Self::Output {
        self.0 * rhs
    }
}

impl PartialEq<u32> for Page {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

impl PartialOrd<u32> for Page {
    fn partial_cmp(&self, other: &u32) -> Option<Ordering> {
        self.0.partial_cmp(other)
    }
}

impl Display for InvalidPage {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("InvalidPage: {}", self.0))
    }
}

impl InvalidPage {
    pub fn message(msg: impl ToString) -> Self {
        Self(msg.to_string())
    }

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

        assert!(p0.is_first());
        assert!(!p1.is_first());
    }
}
