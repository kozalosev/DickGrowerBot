use crate::customization::CustomizationId;

pub(super) fn apply(customization_id: CustomizationId, escaped_text: String) -> String {
    let customization = customization_id.customization();
    let mut decorated = String::with_capacity(
        customization.prefix.len() + escaped_text.len() + customization.suffix.len(),
    );
    decorated.push_str(customization.prefix);
    decorated.push_str(&escaped_text);
    decorated.push_str(customization.suffix);
    decorated
}

#[cfg(test)]
mod tests {
    use crate::customization::{CustomizationId, Decorations};
    use crate::domain::primitives::UserId;

    const UID: UserId = UserId::new(1);

    fn decorations(id: CustomizationId) -> Decorations {
        Decorations::from_entries([(UID, id)])
    }

    #[test]
    fn every_builtin_decoration_has_the_expected_shape() {
        for (id, expected) in [
            (CustomizationId::Crown, "👑 name"),
            (CustomizationId::Fire, "🔥 name 🔥"),
            (CustomizationId::Snake, "🐍 name"),
            (CustomizationId::Eggplant, "🍆 name"),
            (CustomizationId::Banana, "🍌 name"),
            (CustomizationId::Rooster, "🐓 name"),
            (CustomizationId::Cactus, "🌵 name"),
            (CustomizationId::Mushroom, "🍄 name"),
            (CustomizationId::Ruler, "📏 name 📏"),
            (CustomizationId::Growth, "📈 name 📈"),
            (CustomizationId::Muscle, "💪 name 💪"),
            (CustomizationId::Trophy, "🏆 name 🏆"),
            (CustomizationId::Splash, "💦 name 💦"),
            (CustomizationId::Rocket, "🚀 name 🚀"),
            (CustomizationId::Classic, "8===D name"),
            (CustomizationId::Long, "8════════D name"),
            (CustomizationId::Xxl, "[XXL] name"),
            (CustomizationId::Frame, "꧁༺ name ༻꧂"),
            (CustomizationId::Brackets, "『name』"),
            (CustomizationId::Grower, "╰⋃╯ name"),
            (CustomizationId::Bold, "<b>name</b>"),
            (CustomizationId::Monospace, "<code>name</code>"),
        ] {
            assert_eq!(decorations(id).apply(UID, "name".to_owned()), expected);
        }
    }

    #[test]
    fn no_decoration_returns_the_original_text() {
        assert_eq!(Decorations::empty().apply(UID, "name".to_owned()), "name");
    }

    #[test]
    fn an_empty_string_is_safe() {
        assert_eq!(
            decorations(CustomizationId::Fire).apply(UID, String::new()),
            "🔥  🔥"
        );
    }

    #[test]
    fn existing_html_is_preserved_without_a_second_escape() {
        let escaped = "<u>Tom &amp; Jerry</u>".to_owned();
        assert_eq!(
            decorations(CustomizationId::Crown).apply(UID, escaped),
            "👑 <u>Tom &amp; Jerry</u>"
        );
    }

    #[test]
    fn a_decoration_is_applied_once_per_call() {
        let result = decorations(CustomizationId::Snake).apply(UID, "name".to_owned());
        assert_eq!(result.matches('🐍').count(), 1);
    }
}
