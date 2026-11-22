//! The failures of a request that leave nothing to do: the outcome it was after is already there,
//! or nobody is waiting for it any more.

use teloxide::{ApiError, RequestError};

/// Why a failed request is moot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum_macros::IntoStaticStr, strum_macros::VariantArray)]
#[strum(serialize_all = "snake_case")]
pub enum Moot {
    /// The callback or inline query is too old to be answered; the user's client has stopped
    /// waiting for it.
    StaleQuery,
    /// The edit would change nothing, which is what a button pressed twice asks for.
    NotModified,
    /// The message is gone, deleted by somebody or by its own self-destruction.
    MessageGone,
}

impl Moot {
    pub fn of(error: &RequestError) -> Option<Self> {
        use RequestError::Api;
        use ApiError::*;

        match error {
            Api(InvalidQueryId) => Some(Self::StaleQuery),
            Api(MessageNotModified) => Some(Self::NotModified),
            Api(MessageToEditNotFound | MessageToDeleteNotFound | MessageIdInvalid) => Some(Self::MessageGone),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        self.into()
    }
}

pub trait IgnoreMoot {
    /// A moot failure as success, so the caller carries on as if nothing had been asked.
    fn ignore_moot(self) -> Result<(), RequestError>;
}

impl<T> IgnoreMoot for Result<T, RequestError> {
    fn ignore_moot(self) -> Result<(), RequestError> {
        self.map(|_| ())
            .or_else(|error| Moot::of(&error)
                .map(|moot| tracing::debug!(moot = moot.label(), "a request turned out to be moot"))
                .ok_or(error))
    }
}

#[cfg(test)]
mod tests {
    use teloxide::{ApiError, RequestError};
    use super::{IgnoreMoot, Moot};

    #[test]
    fn the_failures_that_leave_nothing_to_do_are_moot() {
        let cases = [
            (ApiError::InvalidQueryId, Moot::StaleQuery),
            (ApiError::MessageNotModified, Moot::NotModified),
            (ApiError::MessageToEditNotFound, Moot::MessageGone),
            (ApiError::MessageToDeleteNotFound, Moot::MessageGone),
            (ApiError::MessageIdInvalid, Moot::MessageGone),
        ];
        for (api, moot) in cases {
            assert_eq!(Moot::of(&RequestError::Api(api)), Some(moot));
        }
    }

    #[test]
    fn a_refusal_is_not_moot() {
        for api in [ApiError::MessageCantBeEdited, ApiError::BotBlocked, ApiError::Unknown("Bad Gateway".to_owned())] {
            let result: Result<(), _> = Err(RequestError::Api(api));
            assert!(result.ignore_moot().is_err());
        }
    }

    #[test]
    fn a_moot_failure_is_ignored() {
        let result: Result<(), _> = Err(RequestError::Api(ApiError::InvalidQueryId));
        let ignored = result.ignore_moot();
        assert!(ignored.is_ok(), "a moot failure must not be an error");
    }
}
