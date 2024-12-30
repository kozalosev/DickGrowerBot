use std::borrow::ToOwned;
use std::ops::Deref;
use derive_more::{Constructor, From};
use once_cell::sync::Lazy;
use teloxide::types::User;

static DEFAULT: Lazy<LanguageCode> = Lazy::new(|| LanguageCode("en".to_string()));

#[derive(Debug, Clone, Constructor, From)]
pub struct LanguageCode(String);

#[derive(Debug, Hash, Copy, Clone, Eq, PartialEq, sqlx::Type)]
#[sqlx(type_name = "language_code", rename_all = "lowercase")]
pub enum SupportedLanguage {
    EN,
    RU,
}

impl LanguageCode {
    pub fn from_user(user: &User) -> Self {
        let maybe_code = Self::get_language_code_or_log_if_missing(user);
        Self::from_maybe_string(maybe_code)
    }

    pub fn from_maybe_user(maybe_user: Option<&User>) -> Self {
        let maybe_code = maybe_user.and_then(Self::get_language_code_or_log_if_missing);
        Self::from_maybe_string(maybe_code)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn to_supported_language(&self) -> SupportedLanguage {
        match self.as_str() {
            "ru" => SupportedLanguage::RU,
            _    => SupportedLanguage::EN
        }
    }

    fn get_language_code_or_log_if_missing(user: &User) -> Option<&String> {
        user.language_code.as_ref()
            .or_else(|| {
                log::debug!("no language_code for {}, using the default", user.id);
                None
            })
    }

    fn from_maybe_string(maybe_string: Option<&String>) -> Self {
        maybe_string
            .map(|code| match &code[..2] {
                "uk" | "be" => "ru",
                _ => code
            })
            .map(ToOwned::to_owned)
            .map(Self)
            .unwrap_or_else(|| DEFAULT.clone())
    }
}

impl Deref for LanguageCode {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<&User> for LanguageCode {
    fn from(value: &User) -> Self {
        Self::from_user(value)
    }
}

impl From<Option<&User>> for LanguageCode {
    fn from(value: Option<&User>) -> Self {
        Self::from_maybe_user(value)
    }
}
