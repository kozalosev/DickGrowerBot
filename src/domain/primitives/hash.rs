#[derive(
    Debug, Clone,
    derive_more::Constructor, derive_more::From,
    PartialEq, Eq,
    sqlx::FromRow, sqlx::Type
)]
pub struct TextHash(Vec<u8>);

impl TextHash {
    pub fn value(&self) -> &[u8] {
        &self.0
    }
}

#[derive(
    Debug, Copy, Clone,
    derive_more::Constructor, derive_more::From
)]
// kept for completeness of the decoded inline_message_id data, even though nothing reads it yet
pub struct AccessHash(#[allow(dead_code)] i64);
