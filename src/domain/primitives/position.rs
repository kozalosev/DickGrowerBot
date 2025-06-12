use domain_types_macro::domain_type;

#[domain_type(
    features(not_database_type)
)]
struct Position(u64);
