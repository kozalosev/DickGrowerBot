use crate::domain::primitives::{Length, Position, UserId, Username};

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

/// What came of an election. `AlreadyChosen` carries today's winner, since that is what the chat
/// is told; `NoDick` means the elected member has nothing to grow.
pub enum DickOfDayResult {
    Chosen(GrowthResult),
    AlreadyChosen(Username),
    NoDick,
}
