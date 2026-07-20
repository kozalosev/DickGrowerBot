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

fn fix_chat_id(number: i64) -> i64 {
    if number.is_negative() {
        // Calculate the chat_id by adding -100 * 10^x to the number
        let power = (number.abs() as f64).log10().floor() as i64 + 1;
        -100 * 10i64.pow(power as u32) + number
    } else {
        number
    }
}

#[cfg(test)]
mod tests {
    use super::fix_chat_id;

    #[test]
    fn test_fix_chat_id() {
        assert_eq!(fix_chat_id(-1100294568), -1001100294568);
        assert_eq!(fix_chat_id(68761694), 68761694);
    }
}
