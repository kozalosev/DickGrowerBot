//! An error that carries the values a failure was about, so that the log can show them as fields
//! and the text of the error stays the same for every failure of its kind.

use std::borrow::{Borrow, Cow};
use std::error::Error;
use std::fmt::{Display, Formatter};
use chrono::{DateTime, NaiveDate, Utc};
use crate::domain::enums::MessageGroup;
use crate::domain::primitives::{AffectedRows, Bet, Count, Debt, LengthChange, Limit, LoanId, LoanPayout, Offset, PerkName, PromoBonus, PromoCode};
use crate::domain::primitives::{SupportedLanguage, TextHash, UserId};
use crate::domain::primitives::chat::{InternalChatId, TelegramChatId, TelegramChatInstanceId, TopicId};
use crate::errors::flat::flat;

pub trait DetailedContext<T> {
    fn detailed_context(self, details: Details) -> anyhow::Result<T>;

    fn user_context(self, uid: UserId, message: impl Into<Cow<'static, str>>) -> anyhow::Result<T>;
}

/// A context of an error, like anyhow's, with the values that stay out of its text. Without an
/// error to wrap, it becomes one by itself through `into()`.
pub struct Details {
    message: Cow<'static, str>,
    fields: ErrorFields,
}

/// Carries the fields under an ordinary context with the message. Its own text is empty, and
/// [`Flatten`](crate::errors::flat::Flatten) leaves it out. It is never the outermost error: anyhow
/// boxes that one as a type of its own, which can't be downcast.
#[derive(Debug, derive_more::Display)]
#[display("")]
struct DetailedError {
    fields: ErrorFields,
    source: Option<anyhow::Error>,
}

/// Every value a failure can name, each logged as a field of its own name. `subject_uid` is the
/// user the operation was for, who isn't necessarily the one who sent the update.
///
/// A value takes its domain type, and is logged as its `value()` or, for `texts`, as its `Display`.
/// `count` takes a `Count` of whatever it counts and keeps only the number, since one field holds a
/// count of any kind.
macro_rules! error_fields {
    (
        values { $($value:ident: $value_type:ty),* $(,)? }
        texts { $($text:ident: $text_type:ty),* $(,)? }
        lists { $($list:ident: $list_type:ty),* $(,)? }
    ) => {
        #[derive(Debug, Default, Clone, PartialEq)]
        pub struct ErrorFields {
            $(pub $value: Option<$value_type>,)*
            pub count: Option<u64>,
            $(pub $text: Option<$text_type>,)*
            $(pub $list: Option<Vec<$list_type>>,)*
        }

        impl Details {
            $(
                pub fn $value(mut self, value: $value_type) -> Self {
                    self.fields.$value = Some(value);
                    self
                }
            )*
            pub fn count<T>(mut self, count: Count<T>) -> Self {
                self.fields.count = Some(*count);
                self
            }
            $(
                pub fn $text(mut self, value: impl Borrow<$text_type>) -> Self {
                    self.fields.$text = Some(value.borrow().clone());
                    self
                }
            )*
            $(
                pub fn $list(mut self, values: impl IntoIterator<Item = $list_type>) -> Self {
                    self.fields.$list = Some(values.into_iter().collect());
                    self
                }
            )*
        }

        impl ErrorFields {
            /// Logs a failed update with these fields beside its error.
            pub fn log_failed_update(&self, error: &(dyn Error + 'static)) {
                tracing::error!(
                    $($value = self.$value.map(|value| value.value()),)*
                    count = self.count,
                    $($text = self.$text.as_ref().map(tracing::field::display),)*
                    $($list = self.$list.as_deref().map(|values| tracing::field::display(Joined(values))),)*
                    error = flat!(error),
                    "an update failed");
            }

            fn or(self, other: &Self) -> Self {
                Self {
                    $($value: self.$value.or(other.$value),)*
                    count: self.count.or(other.count),
                    $($text: self.$text.or_else(|| other.$text.clone()),)*
                    $($list: self.$list.or_else(|| other.$list.clone()),)*
                }
            }
        }
    };
}

error_fields! {
    values {
        subject_uid: UserId,
        offset: Offset,
        limit: Limit,
        topic_id: TopicId,
        loan_id: LoanId,
        internal_chat_id: InternalChatId,
        merged_chat_id: InternalChatId,
        old_chat_id: TelegramChatId,
        new_chat_id: TelegramChatId,
        affected_rows: AffectedRows,
        bet: Bet,
        payout: LoanPayout,
        debt: Debt,
        promo_bonus: PromoBonus,
        length_change: LengthChange,
    }
    texts {
        promo_code: PromoCode,
        date: NaiveDate,
        registered_at: DateTime<Utc>,
        lang_code: SupportedLanguage,
        hash: TextHash,
        cleanup_group: MessageGroup,
        inline_result_id: String,
        merged_chat_instance: TelegramChatInstanceId,
    }
    lists {
        perks: PerkName,
    }
}

impl<T, E: Into<anyhow::Error>> DetailedContext<T> for Result<T, E> {
    fn detailed_context(self, details: Details) -> anyhow::Result<T> {
        self.map_err(|source| details.wrap(Some(source.into())))
    }

    fn user_context(self, uid: UserId, message: impl Into<Cow<'static, str>>) -> anyhow::Result<T> {
        self.detailed_context(Details::new(message)
            .subject_uid(uid))
    }
}

impl Details {
    pub fn new(message: impl Into<Cow<'static, str>>) -> Self {
        Self { message: message.into(), fields: ErrorFields::default() }
    }

    fn wrap(self, source: Option<anyhow::Error>) -> anyhow::Error {
        anyhow::Error::new(DetailedError { fields: self.fields, source }).context(self.message)
    }
}

impl From<Details> for anyhow::Error {
    fn from(details: Details) -> Self {
        details.wrap(None)
    }
}

impl ErrorFields {
    /// The fields of every detailed error in the chain, the outermost winning where two of them
    /// have the same one.
    pub fn of(error: &(dyn Error + 'static)) -> Self {
        std::iter::successors(Some(error), |&e| e.source())
            .filter_map(|e| e.downcast_ref::<DetailedError>())
            .fold(Self::default(), |fields, e| fields.or(&e.fields))
    }
}

impl Error for DetailedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(|source| source.as_ref())
    }
}

/// A list written as its items' text joined by commas.
struct Joined<'a, T>(&'a [T]);

impl<T: Display> Display for Joined<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for (i, value) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            Display::fmt(value, f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use anyhow::Context;
    use crate::domain::primitives::{Count, LengthChange, LoanPayout, UserId};
    use crate::errors::flat::Flatten;
    use super::{DetailedContext, Details, ErrorFields};

    #[test]
    fn the_fields_are_found_under_further_contexts() {
        let error: Box<dyn std::error::Error + Send + Sync> = Err::<(), _>(std::fmt::Error)
            .detailed_context(Details::new("couldn't pay for the loan")
                .payout(LoanPayout::new(5)))
            .user_context(UserId::new(42), "couldn't borrow")
            .context("the outer context")
            .unwrap_err()
            .into();

        let fields = ErrorFields::of(error.as_ref());

        let expected = ErrorFields { subject_uid: Some(UserId::new(42)), payout: Some(LoanPayout::new(5)), ..ErrorFields::default() };
        assert_eq!(fields, expected);
    }

    #[test]
    fn the_outermost_value_wins() {
        let error: Box<dyn std::error::Error + Send + Sync> = Err::<(), _>(std::fmt::Error)
            .user_context(UserId::new(1), "the inner context")
            .user_context(UserId::new(2), "the outer context")
            .unwrap_err()
            .into();

        let fields = ErrorFields::of(error.as_ref());

        assert_eq!(fields.subject_uid, Some(UserId::new(2)));
    }

    #[test]
    fn the_values_stay_out_of_the_text() {
        let error = Err::<(), _>(std::fmt::Error)
            .detailed_context(Details::new("couldn't update the length")
                .subject_uid(UserId::new(42))
                .length_change(LengthChange::signed(-3)))
            .user_context(UserId::new(7), "couldn't borrow")
            .unwrap_err();

        let line = error.flat().to_string();

        assert_eq!(line, "couldn't borrow: couldn't update the length: an error occurred when formatting an argument");
    }

    #[test]
    fn details_without_a_source_are_an_error_of_their_own() {
        let details = Details::new("unexpected count of chats")
            .count(Count::<()>::new(3));
        let error: Box<dyn std::error::Error + Send + Sync> = anyhow::Error::from(details)
            .context("couldn't upsert the chat")
            .into();

        let fields = ErrorFields::of(error.as_ref());
        let line = error.flat().to_string();

        assert_eq!(fields.count, Some(3));
        assert_eq!(line, "couldn't upsert the chat: unexpected count of chats");
    }
}
