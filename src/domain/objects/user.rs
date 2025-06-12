use chrono::{DateTime, Utc};
use macro_rules_attribute::derive;
use crate::domain::primitives::{Length, UserId, Username};

#[derive(sqlx::FromRow, Debug)]
pub struct User {
    pub uid: UserId,
    pub name: Username,
    pub created_at: DateTime<Utc>
}

#[derive(sqlx::FromRow, Debug)]
pub struct ExternalUser {
    pub uid: UserId,
    pub length: Length,
}
