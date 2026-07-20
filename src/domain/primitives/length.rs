use std::ops::Add;
use domain_types::errors::{ArithmeticOperation, DomainArithmeticOverflowError as OverflowError};
use domain_types_macro::domain_type;
use crate::{positive_number, signed_number};

signed_number!(Length, i64);
positive_number!(PositiveLength, i64);
positive_number!(LengthIncrement, i64);

positive_number!(Bet, i32);
positive_number!(LoanPayout, i32);

#[domain_type(number, features(no_auto_display))]
struct SignedLengthChange(i64);

impl std::fmt::Display for SignedLengthChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.sign_plus() && self.0 >= 0 {
            write!(f, "+{}", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, derive_more::Display)]
#[display("{_0}")]
pub enum LengthChange {
    Signed(SignedLengthChange),
    Increment(LengthIncrement),
}

impl LengthChange {
    /// Shorthand for `LengthChange::Signed(SignedLengthChange::new(value))`.
    pub fn signed(value: i64) -> Self {
        Self::Signed(SignedLengthChange::new(value))
    }

    pub fn value(self) -> i64 {
        match self {
            LengthChange::Signed(value) => value.value(),
            LengthChange::Increment(value) => value.value()
        }
    }

    pub fn is_zero(self) -> bool {
        self.value() == 0
    }
}

impl From<SignedLengthChange> for LengthChange {
    fn from(value: SignedLengthChange) -> Self {
        Self::Signed(value)
    }
}

impl From<LengthIncrement> for LengthChange {
    fn from(value: LengthIncrement) -> Self {
        Self::Increment(value)
    }
}

impl Add<SignedLengthChange> for LengthChange {
    type Output = Result<LengthChange, OverflowError<i64>>;

    fn add(self, rhs: SignedLengthChange) -> Self::Output {
        self.value().checked_add(rhs.value())
            .map(SignedLengthChange::new)
            .map(LengthChange::Signed)
            .ok_or(OverflowError::new(ArithmeticOperation::Addition, self.value(), rhs.value()))
    }
}

impl Bet {
    pub fn as_length_change_for_winner(&self) -> LengthChange {
        LengthIncrement::new(self.0.into())
            .map(LengthChange::Increment)
            .expect("Bet is non-negative, so the conversion to LengthIncrement is safe")
    }

    pub fn as_length_change_for_loser(&self) -> LengthChange {
        let value: i64 = self.0.into();
        LengthChange::signed(-value)
    }
}
