use std::borrow::ToOwned;
use once_cell::sync::Lazy;
use teloxide::types::User;
use domain_types_macro::domain_type;

static DEFAULT: Lazy<LanguageCode> = Lazy::new(|| LanguageCode("en".to_string()));
static LOCALE_TO_LANGUAGE: [(&str, SupportedLanguage); 11] = [
    ("en", SupportedLanguage::EN),
    ("ru", SupportedLanguage::RU),
    ("uk", SupportedLanguage::RU),
    ("be", SupportedLanguage::RU),
    ("ky", SupportedLanguage::RU),
    ("kk", SupportedLanguage::RU),
    ("ka", SupportedLanguage::RU),
    ("hy", SupportedLanguage::RU),
    ("it", SupportedLanguage::IT),
    ("fa", SupportedLanguage::FA),
    ("zh", SupportedLanguage::ZH),
];

#[domain_type]
struct LanguageCode(String);

#[derive(Hash, Copy, Clone, Eq, PartialEq, strum_macros::Display, strum_macros::EnumString, sqlx::Type)]
#[strum(serialize_all = "lowercase")]
#[sqlx(type_name = "language_code", rename_all = "lowercase")]
#[cfg_attr(test, derive(Debug))]
pub enum SupportedLanguage {
    EN,
    RU,
    IT,
    FA,
    ZH,
}

impl SupportedLanguage {
    pub const ALL: [SupportedLanguage; 5] = [Self::EN, Self::RU, Self::IT, Self::FA, Self::ZH];

    pub fn flag(&self) -> &'static str {
        match self {
            Self::EN => "🇬🇧",
            Self::RU => "🇷🇺",
            Self::IT => "🇮🇹",
            Self::FA => "🇮🇷",
            Self::ZH => "🇨🇳",
        }
    }

    pub fn native_name(&self) -> &'static str {
        match self {
            Self::EN => "English",
            Self::RU => "Русский",
            Self::IT => "Italiano",
            Self::FA => "فارسی",
            Self::ZH => "中文",
        }
    }

    /// Parses a flag emoji into a language (accepts both 🇬🇧 and 🇺🇸 for English).
    pub fn from_flag(flag: &str) -> Option<Self> {
        Self::ALL.into_iter()
            .find(|lang| lang.flag() == flag)
            .or_else(|| (flag == "🇺🇸").then_some(Self::EN))
    }
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

    pub fn to_supported_language(&self) -> SupportedLanguage {
        self.as_supported_language().unwrap_or(SupportedLanguage::EN)
    }

    /// Resolves this code to a supported language, or `None` if it isn't one we localize.
    /// Unlike [`Self::to_supported_language`], unrecognized codes don't silently fall back to
    /// English — useful when the caller needs to reject unknown input (e.g. the `/language` argument).
    pub fn as_supported_language(&self) -> Option<SupportedLanguage> {
        let code = self.to_ascii_lowercase();
        if code.len() < 2 {
            return None
        }
        LOCALE_TO_LANGUAGE.iter()
            .find(|(locale, _)| *locale == &code[..2])
            .map(|(_, lang)| *lang)
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
    use crate::domain::primitives::SupportedLanguage::{EN, RU, IT, FA, ZH};

    #[test]
    fn success() {
        let ru = [
            "RU", "ru", "Ru", "rU", "ru-RU", "RU-ru", "rU-Ru", "Ru-rU",
            "BE", "be", "Be", "bE", "be-BY", "BE-by", "bE-By", "Be-bY",
            "KY", "ky", "Ky", "kY", "ky-KG", "KY-kg",
            "KK", "kk", "Kk", "kK", "kk-KZ", "KK-kz",
            "KA", "ka", "Ka", "kA", "ka-GE", "KA-ge",
            "HY", "hy", "Hy", "hY", "hy-AM", "HY-am"
        ].map(|code| (code, RU));
        let it = [
            "IT", "it", "It", "iT", "it-IT", "IT-it"
        ].map(|code| (code, IT));
        let fa = [
            "FA", "fa", "Fa", "fA", "fa-IR", "FA-ir"
        ].map(|code| (code, FA));
        let zh = [
            "ZH", "zh", "Zh", "zH", "zh-CN", "ZH-cn", "zh-TW", "ZH-tw"
        ].map(|code| (code, ZH));
        let en = [
            "EN", "en", "En", "eN", "en-US", "EN-us", "eN-Us", "En-uS",
            "c", "C", "POSIX"
        ].map(|code| (code, EN));
        let cases = ru.into_iter().chain(it).chain(fa).chain(zh).chain(en);

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
