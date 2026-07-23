use domain_types_macro::domain_type;
use crate::positive_number;

positive_number!(Counter, i16);

#[domain_type(
    number,
    features(not_database_type)
)]
struct DaysCount(u32);

#[domain_type(
    number,
    division_result(crate::domain::primitives::Ratio),
    features(not_database_type)
)]
struct BattlesCount(u32);

#[domain_type(
    number,
    features(not_database_type)
)]
struct WinStreak(u16);

#[domain_type(
    number,
    features(not_database_type)
)]
struct Position(u64);

#[cfg(test)]
mod deserialize_tests {
    use super::{Counter, DaysCount};

    #[test]
    fn validated_type_round_trips_and_rejects_invalid() {
        let valid: Counter = serde_saphyr::from_str("5").expect("5 must deserialize");
        assert_eq!(valid, Counter::literal(5));

        let invalid = serde_saphyr::from_str::<Counter>("-1");
        assert!(invalid.is_err(), "negative value must be rejected by the validator");
    }

    #[test]
    fn non_validated_type_deserializes_transparently() {
        let days: DaysCount = serde_saphyr::from_str("7").expect("7 must deserialize");
        assert_eq!(days, DaysCount::new(7));
    }
}
