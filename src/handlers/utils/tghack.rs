use std::fmt::Formatter;
use anyhow::anyhow;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use byteorder::{LittleEndian, ReadBytesExt};
use crate::domain::objects::InlineMessageIdInfo;

impl TryFrom<&str> for InlineMessageIdInfo {
    type Error = InvalidIDFormat;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let bytes = URL_SAFE_NO_PAD.decode(value)?;
        let format: IDFormatKind = value.try_into()?;
        let info = format.decode(bytes)?;
        Ok(info)
    }
}

pub fn resolve_inline_message_id(inline_message_id: &str) -> anyhow::Result<InlineMessageIdInfo> {
    log::debug!("inline_message_id: {inline_message_id}");
    let info = inline_message_id.try_into()
        .map_err(|e: InvalidIDFormat| anyhow!(e))?;
    log::debug!("resolved InlineMessageIdInfo: {info:?}");
    Ok(info)
}

#[derive(Debug)]
pub enum InvalidIDFormat {
    DecodeError(base64::DecodeError),
    InvalidLength(String),
    IOError(std::io::Error),
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
                f.write_fmt(format_args!("IdIoError: {err}"))
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
    fn decode(&self, bytes: Vec<u8>) -> Result<InlineMessageIdInfo, std::io::Error> {
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
            fix_chat_id(chat_id),
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

/// Turns the peer id decoded out of an `inline_message_id` into a Bot API `chat_id`.
///
/// Only supergroups and channels come out negative. A legacy (basic) group's inline id doesn't
/// encode the chat at all — it decodes to an unrelated positive number, typically the inline
/// sender's user id — and a positive value is indistinguishable from a basic group's own peer
/// id anyway, so nothing positive can be trusted.
fn fix_chat_id(number: i64) -> Option<i64> {
    number.is_negative()
        .then(|| SUPERGROUP_ID_SHIFT.checked_add(number))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::fix_chat_id;

    #[test]
    fn test_fix_chat_id() {
        // a ten-digit peer id
        assert_eq!(fix_chat_id(-1100294568), Some(-1001100294568));
        // supergroups registered earlier have shorter peer ids and keep the leading zeros
        assert_eq!(fix_chat_id(-599075523), Some(-1000599075523));
        assert_eq!(fix_chat_id(-1234567), Some(-1000001234567));
        // a legacy group decodes to something positive, which is never a chat we can trust
        assert_eq!(fix_chat_id(68761694), None);
        assert_eq!(fix_chat_id(0), None);
        // a value too far out to shift is garbage rather than a chat
        assert_eq!(fix_chat_id(i64::MIN), None);
    }
}
