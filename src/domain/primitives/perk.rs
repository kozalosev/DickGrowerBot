use domain_types_macro::domain_type;
use crate::domain::primitives::validators::perk_name_validator;

/// The `Perks` row a stored state belongs to. Resolved once at startup and only ever used as a key
/// afterwards, so nothing reads a perk's name from the database at runtime.
#[domain_type]
struct PerkId(u16);

/// How a perk calls itself. The same name keys its `Perks` row, its locale strings and its
/// `DISABLE_…` variable.
#[domain_type(
    validated(
        perk_name_validator,
        error_message("must be 1 to 32 ASCII characters"),
    ),
)]
struct PerkName(String);
