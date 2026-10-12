use sqlx::types::JsonValue;
use crate::domain::primitives::PerkId;

/// A perk's state, on its way to the database.
pub struct PerkStateUpdate {
    pub perk_id: PerkId,
    pub state: JsonValue,
}
