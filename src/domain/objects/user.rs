use chrono::{DateTime, Utc};
use crate::domain::primitives::{Length, UserId, Username};

#[derive(sqlx::FromRow, Debug)]
pub struct User {
    pub uid: UserId,
    pub name: Username,
    pub created_at: DateTime<Utc>
}

#[derive(sqlx::FromRow, Debug, PartialEq, derive_more::Constructor)]
pub struct ExternalUser {
    pub uid: UserId,
    pub length: Length,
}
