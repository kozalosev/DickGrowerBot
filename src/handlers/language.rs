use anyhow::anyhow;
use derive_more::Display;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::{CallbackQuery, Message, UserId};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use crate::{check_invoked_by_owner_and_get_answer_params, reply_html};
use crate::domain::primitives::{LanguageCode, SupportedLanguage};
use crate::handlers::{reply_html, HandlerResult};
use crate::handlers::utils::callbacks;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::users::{UserService, UserServiceClient, UserServiceClientGrpc};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum LanguageCommands {
    #[command(description = "language")]
    Language(String),
    Lang(String),
}

impl LanguageCommands {
    fn into_arg(self) -> String {
        match self {
            LanguageCommands::Language(arg) | LanguageCommands::Lang(arg) => arg,
        }
    }
}

pub async fn cmd_handler(bot: Bot, msg: Message, cmd: LanguageCommands,
                         user_service: UserService<UserServiceClientGrpc>, lang_code: LanguageCode) -> HandlerResult {
    let from = msg.from.as_ref().ok_or(anyhow!("unexpected absence of a FROM field"))?;

    let client = match &user_service {
        UserService::Connected(client) => client.clone(),
        UserService::Disabled => {
            reply_html!(bot, msg, t!("commands.language.errors.unavailable", locale = &lang_code));
            return Ok(());
        }
    };

    let arg = cmd.into_arg();
    if arg.trim().is_empty() {
        // No argument — show the inline picker.
        let keyboard = build_language_keyboard(from.id);
        let mut request = reply_html(bot, &msg, t!("commands.language.prompt", locale = &lang_code));
        request.reply_markup = Some(ReplyMarkup::InlineKeyboard(keyboard));
        request.await?;
    } else if let Some(lang) = parse_language_arg(&arg) {
        let text = apply_language(&client, from.id, lang, &lang_code).await?;
        reply_html!(bot, msg, text);
    } else {
        reply_html!(bot, msg, t!("commands.language.errors.unsupported", locale = &lang_code));
    }
    Ok(())
}

#[inline]
pub fn callback_filter(query: CallbackQuery) -> bool {
    LanguageCallbackData::check_prefix(query)
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery,
                              user_service: UserService<UserServiceClientGrpc>, lang_code: LanguageCode) -> HandlerResult {
    let data = LanguageCallbackData::parse(&query)?;
    let answer = check_invoked_by_owner_and_get_answer_params!(bot, query, data.uid);
    let edit_msg_params = callbacks::get_params_for_message_edit(&query)?;

    let text = match user_service {
        UserService::Disabled => t!("commands.language.errors.unavailable", locale = &lang_code).to_string(),
        UserService::Connected(client) => apply_language(&client, data.uid, data.lang, &lang_code).await?,
    };

    callbacks::edit_message_text(&bot, edit_msg_params, text).await?;
    answer.await?;
    Ok(())
}

async fn apply_language(client: &impl UserServiceClient, uid: UserId, lang: SupportedLanguage,
                        current_lang: &LanguageCode) -> anyhow::Result<String> {
    let text = match client.get(uid).await? {
        Some(_) => {
            let code = lang.to_string();
            client.set_language(uid, &code).await?;
            t!("commands.language.success", locale = &code).to_string()
        }
        None => t!("commands.language.not_registered", locale = current_lang).to_string(),
    };
    Ok(text)
}

fn build_language_keyboard(uid: UserId) -> InlineKeyboardMarkup {
    let rows = SupportedLanguage::ALL.iter().map(|&lang| {
        let label = format!("{} {}", lang.flag(), lang.native_name());
        let data = LanguageCallbackData { uid, lang };
        vec![InlineKeyboardButton::callback(label, data.to_data_string())]
    });
    InlineKeyboardMarkup::new(rows)
}

/// Parses a `/language` argument — a flag emoji or a language code (`ru`, `en-US`, `uk`, …) —
/// into a supported language. Returns `None` for anything we don't localize (so the caller can
/// report it). Both the flag and the code folding live on [`SupportedLanguage`]/[`LanguageCode`].
fn parse_language_arg(arg: &str) -> Option<SupportedLanguage> {
    let arg = arg.trim();
    SupportedLanguage::from_flag(arg)
        .or_else(|| LanguageCode::new(arg.to_owned()).as_supported_language())
}

#[derive(Display)]
#[display("{uid}:{lang}")]
pub struct LanguageCallbackData {
    uid: UserId,
    lang: SupportedLanguage,
}

impl CallbackDataWithPrefix for LanguageCallbackData {
    fn prefix() -> &'static str {
        "lang"
    }
}

impl TryFrom<String> for LanguageCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        let mut parts = data.as_str().split(':');
        let uid = callbacks::parse_part(&mut parts, &err, "uid").map(UserId)?;
        let lang = parts.next().ok_or_else(|| err.missing_part("lang"))?;
        let lang = lang.parse().map_err(|_| err.split_err())?;
        Ok(Self { uid, lang })
    }
}

#[cfg(test)]
mod test {
    use teloxide::types::UserId;
    use crate::domain::primitives::SupportedLanguage;
    use crate::handlers::language::{parse_language_arg, LanguageCallbackData};
    use crate::handlers::utils::callbacks::{build_callback_query, CallbackDataWithPrefix};
    use crate::users::mock::UserServiceClientMock;
    use crate::users::generated::{User as ServiceUser, user::Options};
    use crate::users::{UserService, UserServiceClient};

    #[test]
    fn test_parse_language_arg() {
        assert_eq!(parse_language_arg("ru"), Some(SupportedLanguage::RU));
        assert_eq!(parse_language_arg("RU"), Some(SupportedLanguage::RU));
        assert_eq!(parse_language_arg("uk"), Some(SupportedLanguage::RU));
        assert_eq!(parse_language_arg("en-US"), Some(SupportedLanguage::EN));
        assert_eq!(parse_language_arg("🇷🇺"), Some(SupportedLanguage::RU));
        assert_eq!(parse_language_arg("🇬🇧"), Some(SupportedLanguage::EN));
        assert_eq!(parse_language_arg("🇨🇳"), Some(SupportedLanguage::ZH));
        assert_eq!(parse_language_arg("xx"), None);
        assert_eq!(parse_language_arg(""), None);
    }

    #[test]
    fn test_callback_data_roundtrip() {
        let uid = UserId(123456);
        let data = LanguageCallbackData { uid, lang: SupportedLanguage::RU };
        let serialized = data.to_data_string();
        assert_eq!(serialized, "lang:123456:ru");

        let parsed = LanguageCallbackData::parse(&build_callback_query(serialized))
            .expect("callback data must be parsed successfully");
        assert_eq!(parsed.uid, uid);
        assert_eq!(parsed.lang, SupportedLanguage::RU);
    }

    #[tokio::test]
    async fn test_set_language_registered_and_unregistered() {
        let uid = UserId(1);
        let unregistered = UserId(2);
        let mock = match UserServiceClientMock::new() {
            UserService::Connected(client) => client,
            UserService::Disabled => panic!("mock must be connected"),
        };
        mock.insert(uid, ServiceUser {
            id: 42,
            name: Some("tester".to_owned()),
            options: Some(Options::default()),
            is_premium: false,
        });

        mock.set_language(uid, "ru").await.expect("registered user should be updated");
        assert_eq!(mock.language_of(uid).as_deref(), Some("ru"));

        // An unregistered user must not be created.
        assert!(mock.set_language(unregistered, "ru").await.is_err());
        assert!(mock.get(unregistered).await.expect("get must succeed").is_none());
    }
}
