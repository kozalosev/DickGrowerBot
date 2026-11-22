use std::fmt::Formatter;
use anyhow::anyhow;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use byteorder::{LittleEndian, ReadBytesExt};
use crate::domain::objects::InlineMessageIdInfo;
use crate::domain::primitives::chat::TelegramChatId;
use crate::errors::flat::flat;
use crate::metrics;

impl TryFrom<&str> for InlineMessageIdInfo {
    type Error = InvalidIDFormat;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let bytes = URL_SAFE_NO_PAD.decode(value)?;
        let format: IDFormatKind = value.try_into()?;
        let info = format.decode(bytes)?;
        Ok(info)
    }
}

/// Decodes an `inline_message_id`. An id that doesn't decode is logged with its raw value and
/// counted in `unexpected_telegram_ids_total`: Telegram doesn't document the format, so this is
/// where a change to it shows up first.
pub fn resolve_inline_message_id(inline_message_id: &str) -> anyhow::Result<InlineMessageIdInfo> {
    tracing::debug!(inline_message_id = %inline_message_id, "resolving an inline_message_id");
    let info = InlineMessageIdInfo::try_from(inline_message_id)
        .map_err(|e| anyhow!(e))
        .inspect_err(|e| {
            metrics::UNEXPECTED_TELEGRAM_IDS.inline_message_id();
            tracing::error!(error = flat!(e), inline_message_id = %inline_message_id,
                "couldn't resolve the inline_message_id, its format may have changed");
        })?;
    tracing::debug!(info = ?info, "resolved an inline_message_id");
    Ok(info)
}

/// The chat an inline message belongs to, when it can be told at all: only supergroups and
/// channels encode it (see [`fix_chat_id`]). An id that doesn't decode is reported as "unknown".
pub fn try_resolve_chat_id(msg_id: &str) -> Option<TelegramChatId> {
    resolve_inline_message_id(msg_id)
        .ok()
        .and_then(|info| info.chat_id)
}

#[derive(Debug)]
pub enum InvalidIDFormat {
    DecodeError(base64::DecodeError),
    InvalidLength(String),
    IOError(std::io::Error),
    /// A negative peer id too large for a channel or a supergroup.
    PeerOutOfRange(i64),
}

impl From<base64::DecodeError> for InvalidIDFormat {
    fn from(value: base64::DecodeError) -> Self {
        Self::DecodeError(value)
    }
}

impl From<std::io::Error> for InvalidIDFormat {
    fn from(value: std::io::Error) -> Self {
        Self::IOError(value)
    }
}

impl std::fmt::Display for InvalidIDFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            InvalidIDFormat::DecodeError(err) =>
                f.write_fmt(format_args!("IDDecodeError: {err}")),
            InvalidIDFormat::InvalidLength(value) =>
                f.write_fmt(format_args!("InvalidIDLength({}): {value}", value.len())),
            InvalidIDFormat::IOError(err) =>
                f.write_fmt(format_args!("IdIoError: {err}")),
            InvalidIDFormat::PeerOutOfRange(peer) =>
                f.write_fmt(format_args!("PeerOutOfRange: {peer}")),
        }
    }
}

enum IDFormatKind {
    ID32,
    ID64,
}

impl TryFrom<&str> for IDFormatKind {
    type Error = InvalidIDFormat;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.len() {
            27 => Ok(IDFormatKind::ID32),
            32 => Ok(IDFormatKind::ID64),
            _ => Err(InvalidIDFormat::InvalidLength(value.to_owned()))
        }
    }
}

impl IDFormatKind {
    fn decode(&self, bytes: Vec<u8>) -> Result<InlineMessageIdInfo, InvalidIDFormat> {
        let mut cursor = std::io::Cursor::new(bytes);
        let (dc_id, chat_id, message_id, access_hash) = match self {
            IDFormatKind::ID32 => {
                let dc_id = cursor.read_i32::<LittleEndian>()?;
                let message_id = cursor.read_i32::<LittleEndian>()?;
                let chat_id = cursor.read_i32::<LittleEndian>()?;
                let access_hash = cursor.read_i64::<LittleEndian>()?;
                (
                    dc_id,
                    chat_id.into(),
                    message_id,
                    access_hash,
                )
            },
            IDFormatKind::ID64 => (
                cursor.read_i32::<LittleEndian>()?,
                cursor.read_i64::<LittleEndian>()?,
                cursor.read_i32::<LittleEndian>()?,
                cursor.read_i64::<LittleEndian>()?,
            )
        };
        Ok(InlineMessageIdInfo::from_primitive_values(
            dc_id,
            fix_chat_id(chat_id)?,
            message_id,
            access_hash,
        ))
    }
}

/// The Bot API represents a supergroup or a channel as its MTProto peer id shifted into the
/// `-100…` range: `chat_id = -1_000_000_000_000 - peer_id`. The decoded value already carries
/// the peer id negated, so the shift is a plain addition.
///
/// The shift is a constant, *not* a `-100` prefix glued in front of however many digits the
/// peer id happens to have: a peer id shorter than ten digits keeps the leading zeros of the
/// twelve-digit field. Deriving the offset from the number of digits instead only agrees with
/// the rule for exactly ten-digit peer ids, and silently invented a wrong chat for every
/// supergroup registered before those ran out.
const SUPERGROUP_ID_SHIFT: i64 = -1_000_000_000_000;

/// The largest peer id Telegram gives a channel or a supergroup (10¹² − 2³¹). Past it, the Bot
/// API has no `chat_id` for the peer at all.
const MAX_CHANNEL_PEER_ID: i64 = 997_852_516_352;

/// Turns the peer id decoded out of an `inline_message_id` into a Bot API `chat_id`.
///
/// Only supergroups and channels come out negative. A legacy (basic) group's inline id doesn't
/// encode the chat at all — it decodes to an unrelated positive number, typically the inline
/// sender's user id — and a positive value is indistinguishable from a basic group's own peer
/// id anyway, so nothing positive can be trusted.
///
/// A negative value no channel can have means the id isn't laid out the way it is read here, so
/// it is refused rather than turned into a chat that doesn't exist.
fn fix_chat_id(number: i64) -> Result<Option<i64>, InvalidIDFormat> {
    if !number.is_negative() {
        return Ok(None)
    }
    if number < -MAX_CHANNEL_PEER_ID {
        return Err(InvalidIDFormat::PeerOutOfRange(number))
    }
    Ok(Some(SUPERGROUP_ID_SHIFT + number))
}

/// The reverse of the decoding above: an `inline_message_id` of a message posted into the given
/// supergroup, in the ID64 layout (`dc_id`, peer id, `message_id`, `access_hash`) with the peer id
/// negated back out of its Bot API `-100…` form. For tests that need an id the bot can resolve.
#[cfg(test)]
pub fn inline_message_id_of(chat_id: i64) -> String {
    let peer_id = chat_id - SUPERGROUP_ID_SHIFT;
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&1i32.to_le_bytes());       // dc_id
    bytes.extend_from_slice(&peer_id.to_le_bytes());
    bytes.extend_from_slice(&42i32.to_le_bytes());      // message_id
    bytes.extend_from_slice(&7i64.to_le_bytes());       // access_hash
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use crate::domain::primitives::chat::TelegramChatId;
    use super::{fix_chat_id, inline_message_id_of, try_resolve_chat_id, InvalidIDFormat};

    #[test]
    fn test_fix_chat_id() {
        // a ten-digit peer id
        assert_eq!(fix_chat_id(-1100294568).ok(), Some(Some(-1001100294568)));
        // supergroups registered earlier have shorter peer ids and keep the leading zeros
        assert_eq!(fix_chat_id(-599075523).ok(), Some(Some(-1000599075523)));
        assert_eq!(fix_chat_id(-1234567).ok(), Some(Some(-1000001234567)));
        // the last peer id a channel can have
        assert_eq!(fix_chat_id(-997852516352).ok(), Some(Some(-1997852516352)));
        // a legacy group decodes to something positive, which is never a chat we can trust
        assert_eq!(fix_chat_id(68761694).ok(), Some(None));
        assert_eq!(fix_chat_id(0).ok(), Some(None));
    }

    #[test]
    fn peers_out_of_channel_range_are_refused() {
        for peer in [-997852516353, -5_000_000_000_000, i64::MIN] {
            let fixed = fix_chat_id(peer);
            assert!(matches!(fixed, Err(InvalidIDFormat::PeerOutOfRange(p)) if p == peer), "{peer}: {fixed:?}");
        }
    }

    #[test]
    fn an_id_with_a_peer_out_of_range_doesnt_resolve() {
        let resolved = try_resolve_chat_id(&inline_message_id_of(-6_000_000_000_000));
        assert_eq!(resolved, None);
    }

    /// The test fixture has to agree with the decoder it feeds.
    #[test]
    fn test_inline_message_id_round_trip() {
        let chat_id = -1001100294568;
        let resolved = try_resolve_chat_id(&inline_message_id_of(chat_id));
        assert_eq!(resolved, Some(TelegramChatId::new(chat_id)));
    }
}
