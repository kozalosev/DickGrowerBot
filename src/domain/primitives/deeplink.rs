use domain_types_macro::domain_type;
use crate::domain::primitives::validators::start_parameter_validator;

/// What a `t.me/<bot>?start=…` link hands to the bot it opens.
#[domain_type(
    validated(
        start_parameter_validator,
        error_message("must be 1 to 64 Latin letters, digits, underscores or hyphens"),
    ),
)]
struct StartParameter(String);
