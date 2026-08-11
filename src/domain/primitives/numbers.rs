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
