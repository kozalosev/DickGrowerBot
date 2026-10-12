use sqlx::types::JsonValue;
use crate::domain::primitives::{LoanPayout, PerkId};

/// A perk's state, on its way to the database. It travels with the growth that produced it and is
/// written in the same transaction, so a refused growth stores nothing.
pub struct PerkStateUpdate {
    pub perk_id: PerkId,
    pub state: JsonValue,
}

/// Everything the perks want written for one change. It travels with the change and is written in
/// the same transaction, so a refused change stores and pays nothing.
#[derive(Default)]
pub struct PerkWrites {
    pub states: Vec<PerkStateUpdate>,
    pub loan_payout: Option<LoanPayout>,
}
