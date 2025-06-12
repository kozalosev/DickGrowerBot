#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::Constructor)]
#[display("DomainAssertionError for value {}: {}", value, message)]
pub struct DomainAssertionError<T: std::fmt::Display> {
    value: T,
    message: &'static str
}
