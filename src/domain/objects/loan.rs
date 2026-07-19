use crate::domain::primitives::{Debt, Ratio};

#[derive(Debug)]
pub struct Loan {
    pub debt: Debt,
    pub payout_ratio: Ratio,
}
