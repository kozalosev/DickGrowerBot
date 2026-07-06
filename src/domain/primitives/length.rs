use std::ops::Add;
use domain_types::errors::{ArithmeticOperation, DomainArithmeticError, DomainArithmeticOverflowError, DomainAssertionError};
use domain_types::traits::{DomainValue, ValidatedDomainNumber};
use num_traits::CheckedAdd;
use domain_types_macro::domain_type;
use crate::{positive_number, signed_number};
use super::validators::greater_or_equal_to_zero;

signed_number!(Length, i64);
positive_number!(PositiveLength, i64);

signed_number!(SignedLengthChange, i64);
positive_number!(LengthIncrement, i64);

positive_number!(Bet, i32);
positive_number!(LoanPayout, i32);

#[derive(Copy, Clone, derive_more::Display)]
#[display("{_0}")]
pub enum LengthChange {
    Signed(SignedLengthChange),
    Increment(LengthIncrement),
}

impl LengthChange {
    pub(crate) fn new(p0: i32) -> LengthChange {
        todo!()
    }
}

impl LengthChange {
    pub fn value(self) -> i64 {
        match self {
            LengthChange::Signed(value) => value.value(),
            LengthChange::Increment(value) => value.value()
        }
    }
}

impl Add<SignedLengthChange> for LengthChange {
    type Output = Result<LengthChange, DomainArithmeticOverflowError<i64>>;

    fn add(self, rhs: SignedLengthChange) -> Self::Output {
        self.value().checked_add(rhs.value())
            .map(SignedLengthChange::new)
            .map(LengthChange::Signed)
            .ok_or(DomainArithmeticOverflowError::new(ArithmeticOperation::Addition, self.value(), rhs.value()))
    }
}

impl Add<LengthIncrement> for LengthChange {
    type Output = Result<LengthChange, DomainArithmeticError<i64>>;

    fn add(self, rhs: LengthIncrement) -> Self::Output {
        self.value().checked_add(rhs.value())
            .ok_or(DomainArithmeticError::Overflow(
                DomainArithmeticOverflowError::new(ArithmeticOperation::Addition, self.value(), rhs.value())
            ))
            .and_then(LengthIncrement::new)
            .map(LengthChange::Increment)
    }
}

impl Bet {
    pub fn as_length_change_for_winner(&self) -> LengthChange {
        Self::expect_safe_conversion(
            LengthIncrement::new(self.0.into())
                .map(LengthChange::Increment)
        )
    }

    pub fn as_length_change_for_loser(&self) -> LengthChange {
        Self::expect_safe_conversion(
            SignedLengthChange::new(-self.0.into())
                .map(LengthChange::Signed)
        )
    }

    fn expect_safe_conversion(result: Result<LengthChange, DomainAssertionError<i64>>) -> LengthChange {
        result.expect("LengthChange and Bet have the same verification pattern, so conversions are safe")
    }
}
