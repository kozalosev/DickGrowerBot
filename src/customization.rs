use std::collections::HashMap;
use serde::Deserialize;
use domain_types_macro::domain_type;
use crate::domain::primitives::UserId;

#[domain_type]
pub struct CustomizationName(String);

#[derive(Clone, Default)]
pub struct Decorations {
    by_user: HashMap<UserId, Customization>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Customization {
    pub prefix: String,
    pub suffix: String,
}

impl Decorations {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn apply(&self, uid: UserId, escaped_text: String) -> String {
        self.by_user.get(&uid)
            .map(|decoration| [decoration.prefix.as_str(), &escaped_text, decoration.suffix.as_str()].concat())
            .unwrap_or(escaped_text)
    }
}

impl FromIterator<(UserId, Customization)> for Decorations {
    fn from_iter<I: IntoIterator<Item = (UserId, Customization)>>(entries: I) -> Self {
        Self { by_user: entries.into_iter().collect() }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const UID: UserId = UserId::new(1);

    #[test]
    fn test_decoration_preserves_prepared_html() {
        let decorations: Decorations = [(UID, Customization {
            prefix: "👑 ".to_owned(),
            suffix: " 🏆".to_owned(),
        })].into_iter().collect();
        let escaped = "<u>Tom &amp; Jerry</u>".to_owned();
        let decorated = decorations.apply(UID, escaped.clone());
        let plain = decorations.apply(UserId::new(2), escaped.clone());
        let empty = decorations.apply(UID, String::new());
        assert_eq!(decorated, "👑 <u>Tom &amp; Jerry</u> 🏆");
        assert_eq!(plain, escaped);
        assert_eq!(empty, "👑  🏆");
    }

    #[test]
    fn test_empty_decorations() {
        let text = "<u>Tom &amp; Jerry</u>".to_owned();
        let plain = Decorations::empty().apply(UID, text.clone());
        assert_eq!(plain, text);
    }
}
