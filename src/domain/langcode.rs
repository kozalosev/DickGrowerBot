use std::borrow::ToOwned;
use std::error::Error;
use std::fmt::Display;
use std::ops::Deref;
use derive_more::{Constructor, From};
use language_tags::LanguageTag;
use once_cell::sync::Lazy;
use teloxide::types::User;

static DEFAULT: Lazy<LanguageCode> = Lazy::new(|| LanguageCode("en".to_string()));

#[derive(Clone, Constructor, From)]
#[cfg_attr(test, derive(Debug))]
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
        match self.to_ascii_lowercase().as_str() {
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
            .map(|s| s.as_str())
            .and_then(ok_or_log(LanguageTag::parse, "parse language tag"))
            .map(|code: LanguageTag| match code.primary_language() {
                "uk" | "be" => "ru",
                tag => tag
            }.to_owned())
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

fn ok_or_log<T: Display + ?Sized, R, E: Error>(
    mapper: impl FnOnce(&T) -> Result<R, E>,
    action_description: &'static str
) -> impl FnOnce(&T) -> Option<R>
{
    move |value| mapper(value)
        .inspect_err(|e| log::error!("couldn't {action_description} '{value}': {e}"))
        .ok()
}

#[cfg(test)]
mod test_from_maybe_string {
    use crate::domain::LanguageCode;
    use crate::domain::SupportedLanguage::{EN, RU};

    #[test]
    fn success() {
        let ru = [
            "RU", "ru", "Ru", "rU", "ru-RU", "RU-ru", "rU-Ru", "Ru-rU",
            "BE", "be", "Be", "bE", "be-BY", "BE-by", "bE-By", "Be-bY"
        ].map(|code| (code, RU));
        let en = [
            "EN", "en", "En", "eN", "en-US", "EN-us", "eN-Us", "En-uS",
            "c", "C", "POSIX"
        ].map(|code| (code, EN));
        let cases = ru.into_iter().chain(en);

        for (case, expected) in cases {
            let value = case.to_string();
            let result = LanguageCode::from_maybe_string(Some(&value));
            assert_eq!(result.to_supported_language(), expected, "Case: {case}, result: {result:?}")
        }
    }

    #[test]
    fn empty() {
        for case in [Some(&"".to_string()), None] {
            let result = LanguageCode::from_maybe_string(case);
            assert_eq!(result.to_supported_language(), EN)
        }
    }
}
