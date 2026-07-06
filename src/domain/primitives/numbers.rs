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
