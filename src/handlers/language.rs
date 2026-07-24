use autometrics::autometrics;
use std::fmt;
use anyhow::anyhow;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::SendMessageSetters;
use teloxide::prelude::{CallbackQuery, Message, Requester, UserId};
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ReplyMarkup};
use crate::{check_invoked_by_owner_and_get_answer_params, reply_html};
use crate::domain::primitives::chat::ChatIdPartiality;
use crate::domain::primitives::{LanguageCode, SupportedLanguage};
use crate::handlers::{reply_html, HandlerResult};
use crate::handlers::utils::callbacks;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::metrics;
use crate::users::{LanguageService, UserServiceClient};

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

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, user_id = ?crate::handlers::msg_user_id(&msg), lang_code = %lang_code))]
pub async fn cmd_handler(
    bot: Bot,
    msg: Message,
    cmd: LanguageCommands,
    language_service: LanguageService,
    lang_code: LanguageCode,
) -> HandlerResult {
    let from_id = msg.from.as_ref()
        .ok_or(anyhow!("unexpected absence of a FROM field"))?
        .id;
    let arg = cmd.into_arg();

    if msg.chat.is_private() {
        metrics::CMD_LANGUAGE.personal().invoked();
        handle_personal_language(bot, msg, from_id, &arg, language_service, &lang_code).await
    } else {
        metrics::CMD_LANGUAGE.chat().invoked();
        handle_chat_language(bot, msg, from_id, &arg, language_service, &lang_code).await
    }
}

async fn handle_personal_language(
    bot: Bot,
    msg: Message,
    from_id: UserId,
    arg: &str,
    ls: LanguageService,
    lang_code: &LanguageCode,
) -> HandlerResult {
    if !ls.user_service_enabled() {
        reply_html!(bot, msg, t!("commands.language.errors.unavailable", locale = lang_code));
        return Ok(());
    }
    if arg.trim().is_empty() {
        let keyboard = build_language_keyboard(LanguageScope::User, from_id, lang_code);
        let mut request = reply_html(bot.clone(), &msg, t!("commands.language.prompt", locale = lang_code));
        request.reply_markup = Some(ReplyMarkup::InlineKeyboard(keyboard));
        request.await?;
    } else if let Some(lang) = parse_language_arg(arg) {
        let text = apply_user_language(&ls, from_id, lang, lang_code).await?;
        reply_html!(bot, msg, text);
    } else {
        reply_html!(bot, msg, t!("commands.language.errors.unsupported", locale = lang_code));
    }
    Ok(())
}

async fn handle_chat_language(
    bot: Bot,
    msg: Message,
    from_id: UserId,
    arg: &str,
    ls: LanguageService,
    lang_code: &LanguageCode,
) -> HandlerResult {
    if !is_chat_admin(&bot, &msg, from_id).await? {
        reply_html!(bot, msg, t!("commands.language.errors.admins_only", locale = lang_code));
        return Ok(());
    }
    let chat_id: ChatIdPartiality = msg.chat.id.into();
    if arg.trim().is_empty() {
        let keyboard = build_language_keyboard(LanguageScope::Chat, from_id, lang_code);
        reply_html(bot, &msg, t!("commands.language.chat.prompt", locale = lang_code))
            .reply_markup(ReplyMarkup::InlineKeyboard(keyboard))
            .await?;
    } else if let Some(lang) = parse_language_arg(arg) {
        let text = apply_chat_language(&ls, &chat_id, Some(lang), lang_code).await?;
        reply_html!(bot, msg, text);
    } else {
        reply_html!(bot, msg, t!("commands.language.errors.unsupported", locale = lang_code));
    }
    Ok(())
}

async fn is_chat_admin(bot: &Bot, msg: &Message, user_id: UserId) -> anyhow::Result<bool> {
    let admins = bot.get_chat_administrators(msg.chat.id).await?;
    Ok(admins.into_iter().any(|member| member.user.id == user_id))
}

#[inline]
pub fn callback_filter(query: CallbackQuery) -> bool {
    LanguageCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), user_id = query.from.id.0, lang_code = %lang_code))]
pub async fn callback_handler(
    bot: Bot,
    query: CallbackQuery,
    language_service: LanguageService,
    lang_code: LanguageCode,
) -> HandlerResult {
    let data = LanguageCallbackData::parse(&query)?;
    let answer = check_invoked_by_owner_and_get_answer_params!(bot, query, data.uid);
    let edit_msg_params = callbacks::get_params_for_message_edit(&query)?;

    let text = match data.scope {
        LanguageScope::User => match data.selection {
            LanguageSelection::Set(lang) if language_service.user_service_enabled() =>
                apply_user_language(&language_service, data.uid, lang, &lang_code).await?,
            LanguageSelection::Set(_) =>
                t!("commands.language.errors.unavailable", locale = &lang_code).to_string(),
            // The personal picker never offers "Auto"; treat a stray one as unsupported input.
            LanguageSelection::Auto =>
                t!("commands.language.errors.unsupported", locale = &lang_code).to_string(),
        },
        LanguageScope::Chat => {
            let chat_id: ChatIdPartiality = query.message.as_ref()
                .map(|m| m.chat().id.into())
                .ok_or(anyhow!("a chat-language callback without an attached message"))?;
            apply_chat_language(&language_service, &chat_id, data.selection.into_option(), &lang_code).await?
        }
    };

    callbacks::edit_message_text(&bot, edit_msg_params, text).await?;
    answer.await?;
    Ok(())
}

async fn apply_user_language<C: UserServiceClient>(
    ls: &LanguageService<C>,
    uid: UserId,
    lang: SupportedLanguage,
    current_lang: &LanguageCode,
) -> anyhow::Result<String> {
    let text = match ls.user(uid).await? {
        Some(_) => {
            let code = lang.to_string();
            ls.set_user_language(uid, &code).await?;
            metrics::CMD_LANGUAGE.personal().finished();
            t!("commands.language.success", locale = &code).to_string()
        }
        None => t!("commands.language.not_registered", locale = current_lang).to_string(),
    };
    Ok(text)
}

async fn apply_chat_language<C: UserServiceClient>(
    ls: &LanguageService<C>,
    chat_id: &ChatIdPartiality,
    selection: Option<SupportedLanguage>,
    current_lang: &LanguageCode,
) -> anyhow::Result<String> {
    ls.set_chat_language(chat_id, selection).await?;
    metrics::CMD_LANGUAGE.chat().finished();
    let text = match selection {
        Some(lang) => t!("commands.language.chat.success", locale = &lang.to_string()),
        None => t!("commands.language.chat.reset", locale = current_lang),
    }.to_string();
    Ok(text)
}

fn build_language_keyboard(scope: LanguageScope, uid: UserId, lang_code: &LanguageCode) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = SupportedLanguage::ALL.iter().map(|&lang| {
        let label = format!("{} {}", lang.flag(), lang.native_name());
        let data = LanguageCallbackData { scope, uid, selection: LanguageSelection::Set(lang) };
        vec![InlineKeyboardButton::callback(label, data.to_data_string())]
    }).collect();
    // Only the chat-wide picker can revert to per-user resolution.
    if scope == LanguageScope::Chat {
        let label = t!("commands.language.chat.auto_button", locale = lang_code).to_string();
        let data = LanguageCallbackData { scope, uid, selection: LanguageSelection::Auto };
        rows.push(vec![InlineKeyboardButton::callback(label, data.to_data_string())]);
    }
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

#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
enum LanguageScope {
    User,
    Chat,
}

#[derive(Clone, Copy)]
enum LanguageSelection {
    /// Revert to per-user resolution (chat scope only).
    Auto,
    Set(SupportedLanguage),
}

impl LanguageSelection {
    fn into_option(self) -> Option<SupportedLanguage> {
        match self {
            LanguageSelection::Auto => None,
            LanguageSelection::Set(lang) => Some(lang),
        }
    }
}

/// Callback payload of the language picker. The wire format is `lang:<u|c>:<uid>:<code|auto>`:
/// `scope` distinguishes the personal (`u`) from the chat-wide (`c`) setting, `uid` is the invoker
/// (checked on press), and the selection is either a language code or `auto` (chat reset).
pub struct LanguageCallbackData {
    scope: LanguageScope,
    uid: UserId,
    selection: LanguageSelection,
}

impl fmt::Display for LanguageCallbackData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scope = match self.scope {
            LanguageScope::User => "u",
            LanguageScope::Chat => "c",
        };
        let selection = match self.selection {
            LanguageSelection::Auto => "auto".to_owned(),
            LanguageSelection::Set(lang) => lang.to_string(),
        };
        write!(f, "{scope}:{}:{selection}", self.uid)
    }
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
        let scope = match parts.next().ok_or_else(|| err.missing_part("scope"))? {
            "u" => LanguageScope::User,
            "c" => LanguageScope::Chat,
            _ => return Err(err.split_err()),
        };
        let uid = callbacks::parse_part(&mut parts, &err, "uid").map(UserId)?;
        let selection = match parts.next().ok_or_else(|| err.missing_part("selection"))? {
            "auto" => LanguageSelection::Auto,
            code => LanguageSelection::Set(code.parse().map_err(|_| err.split_err())?),
        };
        Ok(Self { scope, uid, selection })
    }
}

#[cfg(test)]
mod test {
    use teloxide::types::UserId;
    use crate::domain::primitives::SupportedLanguage;
    use crate::handlers::language::{parse_language_arg, LanguageCallbackData, LanguageScope, LanguageSelection};
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
        let user_data = LanguageCallbackData { scope: LanguageScope::User, uid, selection: LanguageSelection::Set(SupportedLanguage::RU) };
        assert_eq!(user_data.to_data_string(), "lang:u:123456:ru");

        let parsed = LanguageCallbackData::parse(&build_callback_query("lang:u:123456:ru".to_owned()))
            .expect("user callback data must be parsed successfully");
        assert_eq!(parsed.uid, uid);
        assert_eq!(parsed.scope, LanguageScope::User);
        assert!(matches!(parsed.selection, LanguageSelection::Set(SupportedLanguage::RU)));

        let auto_data = LanguageCallbackData { scope: LanguageScope::Chat, uid, selection: LanguageSelection::Auto };
        assert_eq!(auto_data.to_data_string(), "lang:c:123456:auto");

        let parsed = LanguageCallbackData::parse(&build_callback_query("lang:c:123456:auto".to_owned()))
            .expect("chat callback data must be parsed successfully");
        assert_eq!(parsed.scope, LanguageScope::Chat);
        assert!(matches!(parsed.selection, LanguageSelection::Auto));
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
