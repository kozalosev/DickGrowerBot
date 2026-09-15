use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use crate::domain::primitives::UserId;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CustomizationId {
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

impl CustomizationId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Crown => "crown",
            Self::Fire => "fire",
            Self::Snake => "snake",
            Self::Eggplant => "eggplant",
            Self::Banana => "banana",
            Self::Rooster => "rooster",
            Self::Cactus => "cactus",
            Self::Mushroom => "mushroom",
            Self::Ruler => "ruler",
            Self::Growth => "growth",
            Self::Muscle => "muscle",
            Self::Trophy => "trophy",
            Self::Splash => "splash",
            Self::Rocket => "rocket",
            Self::Classic => "classic",
            Self::Long => "long",
            Self::Xxl => "xxl",
            Self::Frame => "frame",
            Self::Brackets => "brackets",
            Self::Grower => "grower",
            Self::Bold => "bold",
            Self::Monospace => "monospace",
        }
    }

    pub(super) const fn customization(self) -> Customization {
        match self {
            Self::Crown => Customization::new("👑 ", ""),
            Self::Fire => Customization::new("🔥 ", " 🔥"),
            Self::Snake => Customization::new("🐍 ", ""),
            Self::Eggplant => Customization::new("🍆 ", ""),
            Self::Banana => Customization::new("🍌 ", ""),
            Self::Rooster => Customization::new("🐓 ", ""),
            Self::Cactus => Customization::new("🌵 ", ""),
            Self::Mushroom => Customization::new("🍄 ", ""),
            Self::Ruler => Customization::new("📏 ", " 📏"),
            Self::Growth => Customization::new("📈 ", " 📈"),
            Self::Muscle => Customization::new("💪 ", " 💪"),
            Self::Trophy => Customization::new("🏆 ", " 🏆"),
            Self::Splash => Customization::new("💦 ", " 💦"),
            Self::Rocket => Customization::new("🚀 ", " 🚀"),
            Self::Classic => Customization::new("8===D ", ""),
            Self::Long => Customization::new("8════════D ", ""),
            Self::Xxl => Customization::new("[XXL] ", ""),
            Self::Frame => Customization::new("꧁༺ ", " ༻꧂"),
            Self::Brackets => Customization::new("『", "』"),
            Self::Grower => Customization::new("╰⋃╯ ", ""),
            Self::Bold => Customization::new("<b>", "</b>"),
            Self::Monospace => Customization::new("<code>", "</code>"),
        }
    }
}

impl Display for CustomizationId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CustomizationId {
    type Err = strum::ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "crown" => Ok(Self::Crown),
            "fire" => Ok(Self::Fire),
            "snake" => Ok(Self::Snake),
            "eggplant" => Ok(Self::Eggplant),
            "banana" => Ok(Self::Banana),
            "rooster" => Ok(Self::Rooster),
            "cactus" => Ok(Self::Cactus),
            "mushroom" => Ok(Self::Mushroom),
            "ruler" => Ok(Self::Ruler),
            "growth" => Ok(Self::Growth),
            "muscle" => Ok(Self::Muscle),
            "trophy" => Ok(Self::Trophy),
            "splash" => Ok(Self::Splash),
            "rocket" => Ok(Self::Rocket),
            "classic" => Ok(Self::Classic),
            "long" => Ok(Self::Long),
            "xxl" => Ok(Self::Xxl),
            "frame" => Ok(Self::Frame),
            "brackets" => Ok(Self::Brackets),
            "grower" => Ok(Self::Grower),
            "bold" => Ok(Self::Bold),
            "monospace" => Ok(Self::Monospace),
            _ => Err(strum::ParseError::VariantNotFound),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Customization {
    pub prefix: &'static str,
    pub suffix: &'static str,
}

impl Customization {
    const fn new(prefix: &'static str, suffix: &'static str) -> Self {
        Self { prefix, suffix }
    }
}

#[derive(Clone, Default)]
pub struct Decorations {
    by_user: HashMap<UserId, CustomizationId>,
}

impl Decorations {
    pub fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn from_entries(
        entries: impl IntoIterator<Item = (UserId, CustomizationId)>,
    ) -> Self {
        Self {
            by_user: entries.into_iter().collect(),
        }
    }

    pub fn apply(&self, uid: UserId, escaped_text: String) -> String {
        match self.by_user.get(&uid).copied() {
            Some(id) => super::renderer::apply(id, escaped_text),
            None => escaped_text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_stable_database_values() {
        for (id, stored) in [
            (CustomizationId::Crown, "crown"),
            (CustomizationId::Fire, "fire"),
            (CustomizationId::Snake, "snake"),
            (CustomizationId::Eggplant, "eggplant"),
            (CustomizationId::Banana, "banana"),
            (CustomizationId::Rooster, "rooster"),
            (CustomizationId::Cactus, "cactus"),
            (CustomizationId::Mushroom, "mushroom"),
            (CustomizationId::Ruler, "ruler"),
            (CustomizationId::Growth, "growth"),
            (CustomizationId::Muscle, "muscle"),
            (CustomizationId::Trophy, "trophy"),
            (CustomizationId::Splash, "splash"),
            (CustomizationId::Rocket, "rocket"),
            (CustomizationId::Classic, "classic"),
            (CustomizationId::Long, "long"),
            (CustomizationId::Xxl, "xxl"),
            (CustomizationId::Frame, "frame"),
            (CustomizationId::Brackets, "brackets"),
            (CustomizationId::Grower, "grower"),
            (CustomizationId::Bold, "bold"),
            (CustomizationId::Monospace, "monospace"),
        ] {
            assert_eq!(id.as_str(), stored);
            assert_eq!(id.to_string(), stored);
            assert_eq!(
                stored
                    .parse::<CustomizationId>()
                    .expect("the id must parse"),
                id
            );
        }
    }

    #[test]
    fn an_unknown_id_is_rejected() {
        assert!("unknown".parse::<CustomizationId>().is_err());
    }
}
