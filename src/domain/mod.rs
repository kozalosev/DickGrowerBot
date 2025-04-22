mod id;
mod username;
mod ratio;
mod langcode;
mod debt;
mod macros;
mod counter;

pub use id::*;
pub use username::*;
pub use ratio::*;
pub use langcode::*;
pub use debt::*;


#[derive(Debug, derive_more::Display, derive_more::Error)]
pub struct DomainAssertionError(#[error(not(source))] &'static str);
