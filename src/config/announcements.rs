use std::collections::HashMap;
use std::ops::Not;
use std::str::FromStr;
use std::sync::Arc;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use crate::domain::primitives::{Counter, TextHash, LanguageCode, SupportedLanguage};

#[derive(Clone, Default)]
pub struct AnnouncementsConfig {
    pub max_shows: Counter,
    pub announcements: HashMap<SupportedLanguage, Announcement>,
}

impl AnnouncementsConfig {
    pub fn get(&self, lang_code: &LanguageCode) -> Option<&Announcement> {
        self.announcements.get(&lang_code.to_supported_language())
    }

    /// Loads the announcements from a YAML file (see [`AnnouncementsFile`]). A missing file
    /// yields an empty config (the feature is simply off); any read or parse error is logged
    /// and also degrades to an empty config, so a malformed file never crashes the bot.
    pub fn load(path: &str) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::info!("no announcements file at {path}, announcements are disabled");
                return Self::default()
            }
            Err(e) => {
                log::warn!("couldn't read the announcements file at {path}: {e}");
                return Self::default()
            }
        };
        let file: AnnouncementsFile = serde_saphyr::from_str(&content)
            .inspect_err(|e| log::warn!("couldn't parse the announcements file at path {path}: {e}"))
            .unwrap_or_default();
        let announcements = file.texts.into_iter()
            .filter_map(|(code, text)| SupportedLanguage::from_str(&code)
                .inspect_err(|_| log::warn!("unknown language code '{code}' in the announcements file, skipping"))
                .ok().zip(Announcement::new(text.trim().to_owned())))
            .collect();
        Self { max_shows: file.max_shows, announcements }
    }
}

/// The on-disk shape of the announcements file, deserialized from YAML:
///
/// ```yaml
/// max_shows: 5
/// texts:
///   en: |
///     <a href="...">English announcement</a>
///   ru: |
///     <a href="...">Русский текст</a>
/// ```
#[derive(Deserialize, Default)]
struct AnnouncementsFile {
    #[serde(default)]
    max_shows: Counter,
    #[serde(default)]
    texts: HashMap<String, String>,
}

#[derive(Clone)]
pub struct Announcement {
    pub text: Arc<String>,
    pub hash: Arc<TextHash>,
}

impl Announcement {
    pub fn new(text: String) -> Option<Self> {
        text.is_empty().not().then(|| Self  {
            hash: Arc::new(TextHash::new(Sha256::digest(text.as_bytes()).to_vec())),
            text: Arc::new(text),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use crate::domain::primitives::{Counter, SupportedLanguage};
    use super::AnnouncementsConfig;

    /// Writes `content` to a uniquely-named temp file and returns the loaded config.
    fn load_from(content: &str) -> AnnouncementsConfig {
        let path = std::env::temp_dir()
            .join(format!("dgb-announcements-{}.yml", uuid_like()));
        let mut file = std::fs::File::create(&path).expect("couldn't create a temp file");
        file.write_all(content.as_bytes()).expect("couldn't write the temp file");
        let config = AnnouncementsConfig::load(path.to_str().expect("non-UTF-8 temp path"));
        let _ = std::fs::remove_file(&path);
        config
    }

    /// A cheap unique-ish suffix so parallel tests don't clobber each other's temp files.
    fn uuid_like() -> u128 {
        use std::time::{SystemTime, UNIX_EPOCH};
        use std::sync::atomic::AtomicU64;

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128;
        nanos ^ (seq << 100)
    }

    #[test]
    fn valid_file_with_quotes_and_html() {
        let config = load_from(r#"
max_shows: 3
texts:
  en: |
    <a href="https://t.me/kozalo_blog/20">What's new?</a>
  ru: |
    <a href="https://t.me/kozaloru/712">Что нового?</a>
"#);
        assert_eq!(config.max_shows, Counter::literal(3));
        let en = config.announcements.get(&SupportedLanguage::EN)
            .expect("no English announcement");
        assert_eq!(en.text.as_str(), r#"<a href="https://t.me/kozalo_blog/20">What's new?</a>"#);
        let ru = config.announcements.get(&SupportedLanguage::RU)
            .expect("no Russian announcement");
        assert_ne!(en.hash, ru.hash, "distinct texts must hash differently");
    }

    #[test]
    fn comments_are_ignored() {
        let config = load_from(r#"
# a leading comment
max_shows: 2   # inline comment after a value
texts:
  # a comment inside the map
  en: hello    # trailing comment
"#);
        assert_eq!(config.max_shows, Counter::literal(2));
        let en = config.announcements.get(&SupportedLanguage::EN)
            .expect("no English announcement");
        assert_eq!(en.text.as_str(), "hello");
    }

    #[test]
    fn absent_max_shows_defaults_to_zero() {
        let config = load_from("texts:\n  en: hello\n");
        assert_eq!(config.max_shows, Counter::literal(0));
        assert!(config.announcements.contains_key(&SupportedLanguage::EN));
    }

    #[test]
    fn empty_text_is_skipped() {
        let config = load_from("texts:\n  en: hello\n  ru: \"\"\n");
        assert!(config.announcements.contains_key(&SupportedLanguage::EN));
        assert!(!config.announcements.contains_key(&SupportedLanguage::RU));
    }

    #[test]
    fn unknown_language_is_skipped() {
        let config = load_from("texts:\n  en: hello\n  xx: nope\n");
        assert_eq!(config.announcements.len(), 1);
        assert!(config.announcements.contains_key(&SupportedLanguage::EN));
    }

    #[test]
    fn missing_file_yields_empty_config() {
        let config = AnnouncementsConfig::load("definitely/does/not/exist.yml");
        assert_eq!(config.max_shows, Counter::literal(0));
        assert!(config.announcements.is_empty());
    }

    #[test]
    fn malformed_yaml_yields_empty_config() {
        let config = load_from("max_shows: 3\ntexts: [this is not a map");
        assert!(config.announcements.is_empty());
    }

    #[test]
    fn negative_max_shows_yields_empty_config() {
        let config = load_from("max_shows: -1\ntexts:\n  en: hello\n");
        assert!(config.announcements.is_empty());
        assert_eq!(config.max_shows, Counter::literal(0));
    }
}
