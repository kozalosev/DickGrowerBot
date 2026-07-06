use domain_types_macro::domain_type;
use crate::positive_number;
use super::validators::greater_or_equal_to_zero;

positive_number!(Counter, i16);

#[domain_type(
    number,
    features(not_database_type)
)]
struct DaysCount(u32);

#[domain_type(
    number,
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
