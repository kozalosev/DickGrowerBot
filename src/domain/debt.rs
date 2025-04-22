use crate::domain::DomainAssertionError;
use crate::{i64_domain, number_wrapper};

i64_domain!(Debt);

impl From<u32> for Debt {
    fn from(value: u32) -> Self {
        Self(value.into())
    }
}

impl TryFrom<i64> for Debt {
    type Error = DomainAssertionError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value < 0 {
            Err(DomainAssertionError("debt must be positive"))
        } else {
            Ok(Self(value))
        }
    }
}
