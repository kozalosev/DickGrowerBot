use std::collections::HashMap;

use crate::domain::primitives::UserId;

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq,
    strum_macros::Display, strum_macros::EnumString, strum_macros::IntoStaticStr,
)]
#[strum(serialize_all = "lowercase")]
pub enum CustomizationName {
    Crown,
    Fire,
    Snake,
    Eggplant,
    Banana,
    Rooster,
    Cactus,
    Mushroom,
    Ruler,
    Growth,
    Muscle,
    Trophy,
    Splash,
    Rocket,
    Classic,
    Long,
    Xxl,
    Frame,
    Brackets,
    Grower,
    Bold,
    Monospace,
}

#[derive(Clone, Default)]
pub struct Decorations {
    by_user: HashMap<UserId, CustomizationName>,
}

impl Decorations {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn apply(&self, uid: UserId, escaped_text: String) -> String {
        match self.by_user.get(&uid).copied() {
            Some(name) => {
                let customization = Customization::from(name);
                [customization.prefix, &escaped_text, customization.suffix].concat()
            }
            None => escaped_text,
        }
    }
}

impl FromIterator<(UserId, CustomizationName)> for Decorations {
    fn from_iter<I: IntoIterator<Item = (UserId, CustomizationName)>>(entries: I) -> Self {
        Self { by_user: entries.into_iter().collect() }
    }
}

#[derive(Clone, Copy, derive_more::Constructor)]
#[allow(clippy::redundant_field_names, reason = "derive_more::Constructor spells the fields out")]
struct Customization {
    prefix: &'static str,
    suffix: &'static str,
}

impl From<CustomizationName> for Customization {
    fn from(value: CustomizationName) -> Self {
        match value {
            CustomizationName::Crown => Customization::new("👑 ", ""),
            CustomizationName::Fire => Customization::new("🔥 ", " 🔥"),
            CustomizationName::Snake => Customization::new("🐍 ", ""),
            CustomizationName::Eggplant => Customization::new("🍆 ", ""),
            CustomizationName::Banana => Customization::new("🍌 ", ""),
            CustomizationName::Rooster => Customization::new("🐓 ", ""),
            CustomizationName::Cactus => Customization::new("🌵 ", ""),
            CustomizationName::Mushroom => Customization::new("🍄 ", ""),
            CustomizationName::Ruler => Customization::new("📏 ", " 📏"),
            CustomizationName::Growth => Customization::new("📈 ", " 📈"),
            CustomizationName::Muscle => Customization::new("💪 ", " 💪"),
            CustomizationName::Trophy => Customization::new("🏆 ", " 🏆"),
            CustomizationName::Splash => Customization::new("💦 ", " 💦"),
            CustomizationName::Rocket => Customization::new("🚀 ", " 🚀"),
            CustomizationName::Classic => Customization::new("8===D ", ""),
            CustomizationName::Long => Customization::new("8════════D ", ""),
            CustomizationName::Xxl => Customization::new("[XXL] ", ""),
            CustomizationName::Frame => Customization::new("꧁༺ ", " ༻꧂"),
            CustomizationName::Brackets => Customization::new("『", "』"),
            CustomizationName::Grower => Customization::new("╰⋃╯ ", ""),
            CustomizationName::Bold => Customization::new("<b>", "</b>"),
            CustomizationName::Monospace => Customization::new("<code>", "</code>"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: UserId = UserId::new(1);

    #[test]
    fn names_have_stable_database_values() {
        for (name, stored) in [
            (CustomizationName::Crown, "crown"),
            (CustomizationName::Fire, "fire"),
            (CustomizationName::Snake, "snake"),
            (CustomizationName::Eggplant, "eggplant"),
            (CustomizationName::Banana, "banana"),
            (CustomizationName::Rooster, "rooster"),
            (CustomizationName::Cactus, "cactus"),
            (CustomizationName::Mushroom, "mushroom"),
            (CustomizationName::Ruler, "ruler"),
            (CustomizationName::Growth, "growth"),
            (CustomizationName::Muscle, "muscle"),
            (CustomizationName::Trophy, "trophy"),
            (CustomizationName::Splash, "splash"),
            (CustomizationName::Rocket, "rocket"),
            (CustomizationName::Classic, "classic"),
            (CustomizationName::Long, "long"),
            (CustomizationName::Xxl, "xxl"),
            (CustomizationName::Frame, "frame"),
            (CustomizationName::Brackets, "brackets"),
            (CustomizationName::Grower, "grower"),
            (CustomizationName::Bold, "bold"),
            (CustomizationName::Monospace, "monospace"),
        ] {
            assert_eq!(<&str>::from(name), stored);
            assert_eq!(name.to_string(), stored);
            let parsed = stored.parse::<CustomizationName>()
                .expect("the name must parse");
            assert_eq!(parsed, name);
        }
    }

    #[test]
    fn an_unknown_name_is_rejected() {
        assert!("unknown".parse::<CustomizationName>().is_err());
    }

    #[test]
    fn every_builtin_decoration_has_the_expected_shape() {
        for (name, expected) in [
            (CustomizationName::Crown, "👑 name"),
            (CustomizationName::Fire, "🔥 name 🔥"),
            (CustomizationName::Snake, "🐍 name"),
            (CustomizationName::Eggplant, "🍆 name"),
            (CustomizationName::Banana, "🍌 name"),
            (CustomizationName::Rooster, "🐓 name"),
            (CustomizationName::Cactus, "🌵 name"),
            (CustomizationName::Mushroom, "🍄 name"),
            (CustomizationName::Ruler, "📏 name 📏"),
            (CustomizationName::Growth, "📈 name 📈"),
            (CustomizationName::Muscle, "💪 name 💪"),
            (CustomizationName::Trophy, "🏆 name 🏆"),
            (CustomizationName::Splash, "💦 name 💦"),
            (CustomizationName::Rocket, "🚀 name 🚀"),
            (CustomizationName::Classic, "8===D name"),
            (CustomizationName::Long, "8════════D name"),
            (CustomizationName::Xxl, "[XXL] name"),
            (CustomizationName::Frame, "꧁༺ name ༻꧂"),
            (CustomizationName::Brackets, "『name』"),
            (CustomizationName::Grower, "╰⋃╯ name"),
            (CustomizationName::Bold, "<b>name</b>"),
            (CustomizationName::Monospace, "<code>name</code>"),
        ] {
            let decorated = decorations(name).apply(UID, "name".to_owned());
            assert_eq!(decorated, expected);
        }
    }

    #[test]
    fn no_decoration_returns_the_original_text() {
        let text = Decorations::empty().apply(UID, "name".to_owned());
        assert_eq!(text, "name");
    }

    #[test]
    fn an_empty_string_is_safe() {
        let decorated = decorations(CustomizationName::Fire).apply(UID, String::new());
        assert_eq!(decorated, "🔥  🔥");
    }

    #[test]
    fn existing_html_is_preserved_without_a_second_escape() {
        let escaped = "<u>Tom &amp; Jerry</u>".to_owned();
        let decorated = decorations(CustomizationName::Crown).apply(UID, escaped);
        assert_eq!(decorated, "👑 <u>Tom &amp; Jerry</u>");
    }

    #[test]
    fn a_decoration_is_applied_once_per_call() {
        let result = decorations(CustomizationName::Snake).apply(UID, "name".to_owned());
        assert_eq!(result.matches('🐍').count(), 1);
    }

    fn decorations(name: CustomizationName) -> Decorations {
        [(UID, name)].into_iter().collect()
    }
}
