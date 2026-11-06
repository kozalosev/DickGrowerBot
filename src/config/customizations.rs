use std::collections::HashMap;
use teloxide::utils::html::escape;
use crate::customization::{Customization, CustomizationName};

#[derive(Clone, Default)]
pub struct CustomizationsConfig {
    pub customizations: HashMap<CustomizationName, Customization>,
}

impl CustomizationsConfig {
    pub fn get(&self, name: &CustomizationName) -> Option<&Customization> {
        self.customizations.get(name)
    }

    /// Loads a map of names to plain-text prefixes and suffixes. A missing, unreadable or
    /// malformed file yields an empty catalog.
    pub fn load(path: &str) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path, "no customizations file, the decorations are disabled");
                return Self::default()
            }
            Err(e) => {
                tracing::warn!(path = %path, error = %e, "couldn't read the customizations file");
                return Self::default()
            }
        };
        let customizations: HashMap<CustomizationName, Customization> = serde_saphyr::from_str(&content)
            .inspect_err(|e| tracing::warn!(path = %path, error = %e, "couldn't parse the customizations file"))
            .unwrap_or_default();
        let customizations = customizations.into_iter()
            .filter(|(name, _)| {
                let fits = name.chars().count() <= 64;
                if !fits {
                    tracing::warn!(path = %path, customization_name = %name,
                        "skipping a customization name longer than 64 characters");
                }
                fits
            })
            .map(|(name, customization)| (name, Customization {
                prefix: escape(&customization.prefix),
                suffix: escape(&customization.suffix),
            }))
            .collect();
        Self { customizations }
    }
}

#[cfg(test)]
mod test {
    use std::sync::atomic::{AtomicU64, Ordering};
    use crate::customization::Decorations;
    use crate::domain::primitives::UserId;
    use super::*;

    fn load_from(content: &str) -> CustomizationsConfig {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("dgb-customizations-{}-{}.yml",
            std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&path, content).expect("couldn't write the customizations file");
        let config = CustomizationsConfig::load(path.to_str().expect("non-UTF-8 temp path"));
        std::fs::remove_file(path).expect("couldn't remove the customizations file");
        config
    }

    #[test]
    fn test_custom_names_and_optional_affixes() {
        let config = load_from(r#"
# Adding a name requires only a new YAML entry.
new-name:
  prefix: "✨ "
suffix-only:
  suffix: " ✨"
empty: {}
"#);
        let uid = UserId::new(1);
        for (name, expected) in [("new-name", "✨ name"), ("suffix-only", "name ✨"), ("empty", "name")] {
            let customization = config.get(&CustomizationName::of(name)).expect("no configured customization");
            let decorations: Decorations = [(uid, customization.clone())].into_iter().collect();
            let decorated = decorations.apply(uid, "name".to_owned());
            assert_eq!(decorated, expected);
        }
    }

    #[test]
    fn test_affixes_are_plain_text() {
        let config = load_from(r#"
html:
  prefix: "<b>& "
  suffix: " </b>"
"#);
        let uid = UserId::new(1);
        let customization = config.get(&CustomizationName::of("html")).expect("no configured customization");
        let decorations: Decorations = [(uid, customization.clone())].into_iter().collect();
        let decorated = decorations.apply(uid, "<u>Tom &amp; Jerry</u>".to_owned());
        assert_eq!(decorated, "&lt;b&gt;&amp; <u>Tom &amp; Jerry</u> &lt;/b&gt;");
    }

    #[test]
    fn test_names_fit_the_database_column() {
        let longest = "я".repeat(64);
        let too_long = "x".repeat(65);
        let content = format!("{longest}: {{ prefix: '✨ ' }}\n{too_long}: {{ prefix: '🔥 ' }}");
        let config = load_from(&content);
        let valid = config.get(&CustomizationName::of(&longest));
        let invalid = config.get(&CustomizationName::of(&too_long));
        let count = config.customizations.len();
        assert_eq!(count, 1);
        assert!(valid.is_some());
        assert!(invalid.is_none());
    }

    #[test]
    fn test_invalid_files_disable_decorations() {
        for content in ["crown: [", "crown: { prefix: [] }", "crown: { prefex: text }", "{}", ""] {
            let config = load_from(content);
            assert!(config.customizations.is_empty(), "unexpected catalog for {content:?}");
        }
        let config = CustomizationsConfig::load("definitely/does/not/exist.yml");
        assert!(config.customizations.is_empty());
    }

    #[test]
    fn test_default_catalog() {
        let config = CustomizationsConfig::load(concat!(env!("CARGO_MANIFEST_DIR"), "/customizations.yml"));
        let crown = config.get(&CustomizationName::of("crown")).expect("no crown customization");
        let bold = config.get(&CustomizationName::of("bold"));
        let monospace = config.get(&CustomizationName::of("monospace"));
        assert_eq!(crown.prefix, "👑 ");
        assert!(bold.is_none());
        assert!(monospace.is_none());
    }
}
