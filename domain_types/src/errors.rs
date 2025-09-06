use std::fmt::Display;
use num_traits::Num;

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

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::Constructor)]
#[display("DomainArithmeticOverflowError while {} for values {} and {}", operation, op1, op2)]
pub struct DomainArithmeticOverflowError<N: Num> {
    operation: ArithmeticOperation,
    op1: N,
    op2: N,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, strum_macros::Display)]
#[strum(serialize_all = "lowercase")]
pub enum ArithmeticOperation {
    Addition,
    Subtraction,
    Multiplication,
    Division,
    Remainder
}
