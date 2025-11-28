use std::borrow::ToOwned;
use once_cell::sync::Lazy;
use teloxide::types::User;
use domain_types_macro::domain_type;

static DEFAULT: Lazy<LanguageCode> = Lazy::new(|| LanguageCode("en".to_string()));
static RU_SPEAKING_LOCALES: [&str; 3] = ["ru", "uk", "be"];

#[domain_type]
struct LanguageCode(String);

#[derive(Hash, Copy, Clone, Eq, PartialEq, strum_macros::Display, sqlx::Type)]
#[strum(serialize_all = "lowercase")]
#[sqlx(type_name = "language_code", rename_all = "lowercase")]
#[cfg_attr(test, derive(Debug))]
pub enum SupportedLanguage {
    EN,
    RU,
}

impl LanguageCode {

    // TODO: generate by the macro
    pub fn of(value: impl ToString) -> Self {
        Self::new(value.to_string())
    }

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
        let code = self.to_ascii_lowercase();
        if code.len() < 2 {
            SupportedLanguage::EN
        } else if RU_SPEAKING_LOCALES.contains(&&code[..2]) {
            SupportedLanguage::RU
        } else {
            SupportedLanguage::EN
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
            .map(ToOwned::to_owned)
            .map(Self)
            .unwrap_or_else(|| DEFAULT.clone())
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

#[cfg(test)]
mod test_from_maybe_string {
    use crate::domain::primitives::LanguageCode;
    use crate::domain::primitives::SupportedLanguage::{EN, RU};

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
