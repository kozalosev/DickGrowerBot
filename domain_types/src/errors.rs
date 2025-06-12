use std::fmt::Display;

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::Constructor)]
#[display("DomainAssertionError for value {}: {}", value, message)]
pub struct DomainAssertionError<T: Display> {
    value: T,
    message: &'static str
}

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::Constructor)]
#[display("DomainParseError for value {}: cannot parse the string into a {} (cause: {})", value, type_name, error)]
pub struct DomainParseError {
    value: String,
    type_name: &'static str,
    error: Box<dyn std::error::Error + Send + Sync + 'static>,
}
