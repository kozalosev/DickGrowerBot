use crate::domain::primitives::{Length, Position, UserId};

#[derive(Debug)]
pub struct Dick {
    pub length: Length,
    pub owner_uid: UserId,
    pub owner_name: String,
    pub grown_at: chrono::DateTime<chrono::Utc>,
    pub position: Option<Position>,
}

pub struct GrowthResult {
    pub new_length: Length,
    pub pos_in_top: Option<Position>,
}

/// What came of an election that was won. `NoDick` means the winner's dick no longer existed when
/// the bonus was to be credited, which is not the same as an election with no candidates.
pub enum DickOfDayResult {
    Chosen(GrowthResult),
    NoDick,
}
