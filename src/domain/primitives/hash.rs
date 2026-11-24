#[derive(
    Debug, Clone,
    derive_more::Constructor, derive_more::From,
    PartialEq, Eq,
    sqlx::Type
)]
#[sqlx(transparent)]
pub struct TextHash(Vec<u8>);

impl std::fmt::Display for TextHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

#[derive(
    Debug, Copy, Clone,
    derive_more::Constructor, derive_more::From
)]
// kept for completeness of the decoded inline_message_id data, even though nothing reads it yet
pub struct AccessHash(#[allow(dead_code)] i64);
