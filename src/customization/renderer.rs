use crate::customization::CustomizationName;
use crate::customization::model::Customization;

pub(super) fn apply(customization_name: CustomizationName, escaped_text: String) -> String {
    let customization = Customization::from(customization_name);
    [customization.prefix, &escaped_text, customization.suffix].concat()
}

#[cfg(test)]
mod tests {
    use crate::customization::{CustomizationName, Decorations};
    use crate::domain::primitives::UserId;

    const UID: UserId = UserId::new(1);
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
            assert_eq!(decorations(name).apply(UID, "name".to_owned()), expected);
        }
    }

    #[test]
    fn no_decoration_returns_the_original_text() {
        assert_eq!(Decorations::empty().apply(UID, "name".to_owned()), "name");
    }

    #[test]
    fn an_empty_string_is_safe() {
        assert_eq!(
            decorations(CustomizationName::Fire).apply(UID, String::new()),
            "🔥  🔥"
        );
    }

    #[test]
    fn existing_html_is_preserved_without_a_second_escape() {
        let escaped = "<u>Tom &amp; Jerry</u>".to_owned();
        assert_eq!(
            decorations(CustomizationName::Crown).apply(UID, escaped),
            "👑 <u>Tom &amp; Jerry</u>"
        );
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
