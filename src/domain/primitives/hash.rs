#[derive(
    Debug, Clone,
    derive_more::Constructor, derive_more::From,
    sqlx::FromRow, sqlx::Type
)]
pub struct TextHash(Vec<u8>);

#[derive(
    Debug, Copy, Clone,
    derive_more::Constructor, derive_more::From
)]
pub struct AccessHash(i64);
