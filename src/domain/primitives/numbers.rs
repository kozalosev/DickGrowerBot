use domain_types::traits::SaturatingInto;
use domain_types_macro::domain_type;
use crate::number;

number!(Counter, u16);

#[domain_type(number)]
struct DaysCount(u32);

#[domain_type(
    number,
    division_result(crate::domain::primitives::Ratio),
)]
struct BattlesCount(u32);

#[domain_type(number)]
struct WinStreak(u16);

#[domain_type(number)]
struct Position(u64);

#[domain_type(number)]
struct AffectedRows(u64);

impl AffectedRows {
    pub fn zero(&self) -> bool {
        self.0 == 0
    }

    pub fn single(&self) -> bool {
        self.0 == 1
    }

    pub fn several(&self) -> bool {
        self.0 > 1
    }
}

#[domain_type(number)]
struct AttemptsCount(u32);

/// How many requests are allowed in one period of time. Which period, and to whom, is up to the
/// field that holds it.
#[domain_type(number)]
struct RateLimit(u32);

/// How long a message lives before it is taken away. Zero means it is never taken away at all,
/// which is what makes this a count of minutes rather than a `Duration`: the value is chosen from
/// a list, stored in jsonb and shown on a button, and all three want a plain number.
#[domain_type(number)]
struct DelayMinutes(u32);

/// The visible characters of a message — what its reading time is estimated from. A Telegram
/// message holds at most a few thousand of them, so the width is never in question.
#[domain_type(number)]
struct CharCount(u32);

impl CharCount {
    /// What a reader actually gets through: characters, not bytes.
    pub fn of(text: &str) -> Self {
        Self(text.chars().count().saturating_into())
    }
}

#[cfg(test)]
mod deserialize_tests {
    use super::{Counter, DaysCount};

    #[test]
    fn validated_type_round_trips_and_rejects_invalid() {
        let valid: Counter = serde_saphyr::from_str("5").expect("5 must deserialize");
        assert_eq!(valid, Counter::new(5));

        let invalid = serde_saphyr::from_str::<Counter>("-1");
        assert!(invalid.is_err(), "negative value must be rejected by the validator");
    }

    #[test]
    fn non_validated_type_deserializes_transparently() {
        let days: DaysCount = serde_saphyr::from_str("7").expect("7 must deserialize");
        assert_eq!(days, DaysCount::new(7));
    }
}
