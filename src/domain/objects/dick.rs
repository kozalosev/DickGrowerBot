use crate::domain::primitives::{Length, Position, UserId};

#[derive(sqlx::FromRow, Debug)]
pub struct Dick {
    pub length: Length,
    pub owner_uid: UserId,
    pub owner_name: String,
    pub grown_at: chrono::DateTime<chrono::Utc>,
    pub position: Option<i64>,
}

pub struct GrowthResult {
    pub new_length: Length,
    pub pos_in_top: Option<Position>,
}
