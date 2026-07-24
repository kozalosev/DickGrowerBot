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
        let base: HashMap<SupportedLanguage, Announcement> = file.texts.into_iter()
            .filter_map(|(code, text)| SupportedLanguage::from_str(&code)
                .inspect_err(|_| log::warn!("unknown language code '{code}' in the announcements file, skipping"))
                .ok().zip(Announcement::new(text.trim().to_owned())))
            .collect();
        let announcements = Self::apply_fallback(base, file.fallback);
        Self { max_shows: file.max_shows, announcements }
    }

    /// Fills gaps in the per-language announcement map from the `fallback` config: for each
    /// `source -> [targets]` entry, any target that has no announcement of its own borrows the
    /// source's. Sources are resolved from the immutable `base` snapshot, so fallback is
    /// non-transitive and independent of `HashMap` iteration order. A target listed under two
    /// different sources resolves to one of them nondeterministically — list each target under a
    /// single source.
    fn apply_fallback(
        base: HashMap<SupportedLanguage, Announcement>,
        fallback: HashMap<String, Vec<String>>,
    ) -> HashMap<SupportedLanguage, Announcement> {
        let mut announcements = base.clone();
        for (source_code, target_codes) in fallback {
            let source_ann = SupportedLanguage::from_str(&source_code).ok()
                .and_then(|lang| base.get(&lang));
            let Some(source_ann) = source_ann else {
                // unknown source code, or the source has no announcement of its own — nothing to lend
                continue
            };
            for target_code in target_codes {
                match SupportedLanguage::from_str(&target_code) {
                    Ok(target) => { announcements.entry(target).or_insert_with(|| source_ann.clone()); }
                    Err(_) => log::warn!("unknown fallback target language '{target_code}' in the announcements file, skipping"),
                }
            }
        }
        announcements
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
/// # show the English text to it/fa/zh when they have no announcement of their own
/// fallback:
///   en: [it, fa, zh]
/// ```
#[derive(Deserialize, Default)]
struct AnnouncementsFile {
    #[serde(default)]
    max_shows: Counter,
    #[serde(default)]
    texts: HashMap<String, String>,
    /// Maps a source language to the target languages that borrow its text when they have no
    /// announcement of their own. See [`AnnouncementsConfig::apply_fallback`].
    #[serde(default)]
    fallback: HashMap<String, Vec<String>>,
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
        let config = load_from(r#"
texts:
  en: hello
"#);
        assert_eq!(config.max_shows, Counter::literal(0));
        assert!(config.announcements.contains_key(&SupportedLanguage::EN));
    }

    #[test]
    fn empty_text_is_skipped() {
        let config = load_from(r#"
texts:
  en: hello
  ru: ""
"#);
        assert!(config.announcements.contains_key(&SupportedLanguage::EN));
        assert!(!config.announcements.contains_key(&SupportedLanguage::RU));
    }

    #[test]
    fn unknown_language_is_skipped() {
        let config = load_from(r#"
texts:
  en: hello
  xx: nope
"#);
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
        let config = load_from(r#"
max_shows: 3
texts: [this is not a map
"#);
        assert!(config.announcements.is_empty());
    }

    #[test]
    fn negative_max_shows_yields_empty_config() {
        let config = load_from(r#"
max_shows: -1
texts:
  en: hello
"#);
        assert!(config.announcements.is_empty());
        assert_eq!(config.max_shows, Counter::literal(0));
    }

    mod fallback {
        use super::*;
        use crate::domain::primitives::SupportedLanguage::*;

        #[test]
        fn fills_all_missing_targets() {
            let config = load_from(r#"
texts:
  en: hello
fallback:
  en: [it, fa, zh]
"#);
            for lang in [EN, IT, FA, ZH] {
                let ann = config.announcements.get(&lang)
                    .unwrap_or_else(|| panic!("no announcement for {lang:?}"));
                assert_eq!(ann.text.as_str(), "hello");
            }
            assert!(!config.announcements.contains_key(&RU));
        }

        #[test]
        fn partial_coverage_flow_list() {
            let config = load_from(r#"
texts:
  en: hello
fallback:
  en: [it, fa]
"#);
            assert!(config.announcements.contains_key(&IT));
            assert!(config.announcements.contains_key(&FA));
            assert!(!config.announcements.contains_key(&ZH), "zh isn't listed, must stay absent");
        }

        #[test]
        fn block_sequence_syntax() {
            let config = load_from(r#"
texts:
  en: hello
fallback:
  en:
    - it
    - fa
"#);
            assert!(config.announcements.contains_key(&IT));
            assert!(config.announcements.contains_key(&FA));
            assert!(!config.announcements.contains_key(&ZH));
        }

        #[test]
        fn shares_source_hash() {
            let config = load_from(r#"
texts:
  en: hello
fallback:
  en: [it]
"#);
            let en = config.announcements.get(&EN).expect("no English announcement");
            let it = config.announcements.get(&IT).expect("no Italian announcement");
            assert_eq!(it.text, en.text, "borrowed text must equal the source's");
            assert_eq!(it.hash, en.hash, "borrowed hash must equal the source's");
        }

        #[test]
        fn does_not_override_own_text() {
            let config = load_from(r#"
texts:
  en: hello
  it: ciao
fallback:
  en: [it]
"#);
            let it = config.announcements.get(&IT).expect("no Italian announcement");
            assert_eq!(it.text.as_str(), "ciao", "a language with its own text keeps it");
        }

        #[test]
        fn source_without_text_lends_nothing() {
            let config = load_from(r#"
texts:
  ru: привет
fallback:
  en: [it]
"#);
            assert!(!config.announcements.contains_key(&IT), "source has no text, nothing to lend");
        }

        #[test]
        fn unknown_codes_are_skipped() {
            let config = load_from(r#"
texts:
  en: hello
fallback:
  en: [it, zzz]
  yyy: [fa]
"#);
            assert!(config.announcements.contains_key(&IT), "valid target still applied");
            assert!(!config.announcements.contains_key(&FA), "unknown source is skipped");
        }

        #[test]
        fn is_non_transitive() {
            let config = load_from(r#"
texts:
  en: hello
fallback:
  en: [ru]
  ru: [it]
"#);
            assert!(config.announcements.contains_key(&RU), "ru borrows en directly");
            assert!(!config.announcements.contains_key(&IT), "it must not chain through ru's borrowed text");
        }

        #[test]
        fn no_fallback_key_behaves_as_before() {
            let config = load_from(r#"
texts:
  en: hello
  ru: привет
"#);
            assert_eq!(config.announcements.len(), 2);
            assert!(config.announcements.contains_key(&EN));
            assert!(config.announcements.contains_key(&RU));
        }
    }
}
