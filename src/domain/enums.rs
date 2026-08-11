//! Closed sets of domain values: neither primitives wrapping something else, nor objects made of
//! several fields, but names the whole bot agrees on.

/// Lifetime category of a bot message, used to decide when (if ever) it self-destructs.
///
/// * `Notice` = canned, always-the-same messages (help, privacy, errors, statuses);
/// * `Report` = generated read-outs (leaderboard, stats);
/// * `Event` = permanent records (growths, DoDs, fights);
/// * `Application` = interactive requests (loans, battles).
///
/// The lowercase spelling is shared by the `message_group` enum of the database, the label of
/// [`crate::metrics::SELF_DESTRUCTION`] and the log field, so the three can't drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, sqlx::Type,
         strum_macros::Display, strum_macros::EnumString, strum_macros::EnumIter)]
#[strum(serialize_all = "lowercase")]
#[sqlx(type_name = "message_group", rename_all = "lowercase")]
pub enum MessageGroup {
    Notice,
    Report,
    Event,
    Application,
}
