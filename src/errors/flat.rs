//! An error flattened onto one line, for logging it as an error rather than as a string.
//!
//! A field recorded as `&dyn Error` becomes `exception.message` in the exported records and an
//! exception on the span. The log exporter writes only the error's `Display` and never walks
//! `source()`, so the cause chain has to be in the `Display` itself, and it has to stay on one line:
//! the log database stores a record per line.
//!
//! ```ignore
//! tracing::warn!(error = flat!(e), "couldn't fetch the settings of the chat");
//! ```

use std::error::Error;

/// Turns an error into the `&dyn Error` a `tracing` field records as an error.
macro_rules! flat {
    ($error:expr) => {
        &{
            use $crate::errors::flat::Flatten as _;
            ($error).flat()
        } as &dyn ::std::error::Error
    };
}
pub(crate) use flat;

#[derive(derive_more::Display, derive_more::Debug, derive_more::Error)]
#[debug("{_0:?}")]
pub struct FlatError(#[error(not(source))] String);

pub trait Flatten {
    fn flat(&self) -> FlatError;
}

/// Covers `anyhow::Error` too, through its `Deref` to `dyn Error`, whose `source()` is the chain of
/// contexts. A link with no text of its own is left out.
impl<E: Error + ?Sized> Flatten for E {
    fn flat(&self) -> FlatError {
        let mut line = self.to_string();
        let mut cause = self.source();
        while let Some(e) = cause {
            let text = e.to_string();
            if !text.is_empty() {
                if !line.is_empty() {
                    line.push_str(": ");
                }
                line.push_str(&text);
            }
            cause = e.source();
        }
        FlatError(line)
    }
}

#[cfg(test)]
mod test {
    use anyhow::Context;
    use super::Flatten;

    #[test]
    fn an_anyhow_chain_stays_on_one_line() {
        let error = Err::<(), _>(std::fmt::Error)
            .context("the inner context")
            .context("the outer context")
            .unwrap_err();

        let line = error.flat().to_string();

        assert_eq!(line, "the outer context: the inner context: an error occurred when formatting an argument");
        assert_eq!(line, format!("{error:#}"));
    }

    #[test]
    fn a_boxed_chain_stays_on_one_line() {
        let error: Box<dyn std::error::Error + Send + Sync> = Err::<(), _>(std::fmt::Error)
            .context("the context")
            .unwrap_err()
            .into();

        let line = error.flat().to_string();

        assert_eq!(line, "the context: an error occurred when formatting an argument");
    }
}
