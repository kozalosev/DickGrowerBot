use std::str::{FromStr, Split};
use derive_more::{Display, Error};
use rust_i18n::t;
use teloxide::Bot;
use teloxide::payloads::AnswerCallbackQuery;
use teloxide::prelude::ChatId;
use teloxide::requests::{JsonRequest, Requester};
use teloxide::types::{CallbackQuery, MessageId, UserId};

use crate::handlers::ensure_lang_code;
use crate::repo::ChatIdKind;

#[derive(Debug, Display, Error)]
pub enum InvalidCallbackData {
    NoData,
    #[display("WrongPrefix(data={data}, prefix={prefix})")]
    WrongPrefix { data: String, prefix: String },
    #[display("SplitError(data={data})")]
    SplitError { data: String },
    #[display("MissingPart(data={data}, part={part})")]
    MissingPart { data: String, part: String },
    #[display("InvalidFormat(data={data}, error={error})")]
    InvalidFormat { data: String, error: Box<dyn std::error::Error + Send + Sync> },
}

/// Type for new fields which is not present in old messages.
#[derive(Display)]
pub enum NewLayoutValue<T> {
    Some(T),

    // may appear in case of re-serialization of an old deserialized message
    #[display("OLDVER")]
    None
}

impl <T> From<Option<T>> for NewLayoutValue<T> {
    fn from(value: Option<T>) -> Self {
        match value {
            None => NewLayoutValue::None,
            Some(x) => NewLayoutValue::Some(x)
        }
    }
}

pub struct InvalidCallbackDataBuilder<'a, T: ToString>(pub &'a T);

impl <'a, T: ToString> InvalidCallbackDataBuilder<'a, T> {
    pub fn split_err(&self) -> InvalidCallbackData {
        InvalidCallbackData::SplitError {
            data: self.0.to_string()
        }
    }

    pub fn wrong_prefix(&self, prefix: impl ToString) -> InvalidCallbackData {
        InvalidCallbackData::WrongPrefix {
            data: self.0.to_string(),
            prefix: prefix.to_string()
        }
    }

    pub fn missing_part(&self, part: &str) -> InvalidCallbackData {
        InvalidCallbackData::MissingPart {
            data: self.0.to_string(),
            part: part.to_owned()
        }
    }

    pub fn parsing_err(&self, err: impl std::error::Error + Send + Sync + 'static) -> InvalidCallbackData {
        InvalidCallbackData::InvalidFormat {
            data: self.0.to_string(),
            error: Box::new(err)
        }
    }
}

pub trait CallbackDataWithPrefix<E = InvalidCallbackData>: TryFrom<String, Error = E> + std::fmt::Display
    where E: std::error::Error + Send + Sync + 'static
{
    fn prefix() -> &'static str;
    
    fn check_prefix(query: CallbackQuery) -> bool {
        query.data
            .filter(|data| data.starts_with(Self::prefix()))
            .is_some()
    }
    
    fn parse(query: &CallbackQuery) -> Result<Self, InvalidCallbackData> {
        let data = query.data.as_ref().ok_or(InvalidCallbackData::NoData)?;
        let err = InvalidCallbackDataBuilder(data);
        let value = match data.split_once(':') {
            Some((prefix, rest)) if prefix == Self::prefix() => Ok(rest.to_owned()),
            Some((prefix, _)) => Err(err.wrong_prefix(prefix)),
            None => Err(InvalidCallbackData::NoData)
        }?;
        Self::try_from(value).map_err(|e| err.parsing_err(e))
    }
    
    fn to_data_string(&self) -> String {
        format!("{}:{}", Self::prefix(), self)
    }
}

#[derive(Clone)]
pub enum EditMessageReqParamsKind {
    Chat(ChatId, MessageId),
    Inline { chat_instance: String, inline_message_id: String },
}

#[allow(clippy::from_over_into)]
impl Into<ChatIdKind> for EditMessageReqParamsKind {
    fn into(self) -> ChatIdKind {
        match self {
            EditMessageReqParamsKind::Chat(chat_id, _) => chat_id.into(),
            EditMessageReqParamsKind::Inline { chat_instance, .. } => chat_instance.into(),
        }
    }
}

pub fn get_params_for_message_edit(q: &CallbackQuery) -> Result<EditMessageReqParamsKind, &'static str> {
    q.message.as_ref()
        .map(|m| EditMessageReqParamsKind::Chat(m.chat.id, m.id))
        .or(q.inline_message_id.as_ref().map(|inline_message_id| EditMessageReqParamsKind::Inline {
            chat_instance: q.chat_instance.clone(),
            inline_message_id: inline_message_id.clone()
        }))
        .ok_or("no message")
}

pub enum CallbackAnswerParams {
    Answer { answer: JsonRequest<AnswerCallbackQuery>, lang_code: String },
    AnotherUser,
}

pub async fn prepare_callback_answer_params(bot: &Bot, query: &CallbackQuery, user_id: UserId) -> Result<CallbackAnswerParams, teloxide::RequestError> {
    let lang_code = ensure_lang_code(Some(&query.from));
    let mut answer = bot.answer_callback_query(&query.id);
    let res = if query.from.id != user_id {
        answer.show_alert.replace(true);
        answer.text.replace(t!("inline.callback.errors.another_user", locale = &lang_code));
        answer.await?;
        CallbackAnswerParams::AnotherUser
    } else {
        CallbackAnswerParams::Answer{ answer, lang_code }
    };
    Ok(res)
}

/// Utility method to make easier to implement the CallbackDataWithPrefix::parse() method.
pub fn parse_part<VT, PDT>(parts: &mut Split<char>, err_builder: &InvalidCallbackDataBuilder<VT>, part_name: &str) -> Result<PDT, InvalidCallbackData>
where
    VT: ToString,
    PDT: FromStr,
    <PDT as FromStr>::Err: std::error::Error + Send + Sync + 'static
{
    parts.next()
        .ok_or_else(|| err_builder.missing_part(part_name))
        .and_then(|uid| uid.parse().map_err(|e| err_builder.parsing_err(e)))
}

pub fn parse_optional_part<VT, PDT>(parts: &mut Split<char>, err_builder: &InvalidCallbackDataBuilder<VT>) -> Result<NewLayoutValue<PDT>, InvalidCallbackData>
where
    VT: ToString,
    PDT: FromStr,
    <PDT as FromStr>::Err: std::error::Error + Send + Sync + 'static
{
    parts.next()
        .filter(|x| x != &"OLDVER")
        .map(|x| x.parse())
        .transpose()
        .map(NewLayoutValue::from)
        .map_err(|e| err_builder.parsing_err(e))
}

#[macro_export]
macro_rules! check_invoked_by_owner_and_get_answer_params {
    ($bot:ident, $query:ident, $user_id:expr) => {
        match $crate::handlers::utils::callbacks::prepare_callback_answer_params(&$bot, &$query, $user_id).await? {
            $crate::handlers::utils::callbacks::CallbackAnswerParams::Answer { answer, lang_code } => (answer, lang_code),
            $crate::handlers::utils::callbacks::CallbackAnswerParams::AnotherUser => return Ok(()),
        }
    }
}
