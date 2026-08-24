use sqlx::types::JsonValue;
use crate::domain::primitives::PerkId;

/// A perk's state, on its way to the database. It travels with the growth that produced it and is
/// written in the same transaction, so a refused growth stores nothing.
pub struct PerkStateUpdate {
    pub perk_id: PerkId,
    pub state: JsonValue,
}
