use crate::domain::primitives::{Debt, PayoutRatio};

#[derive(Debug)]
pub struct Loan {
    pub debt: Debt,
    pub payout_ratio: PayoutRatio,
}
