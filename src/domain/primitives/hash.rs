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
pub struct AccessHash(i64);
