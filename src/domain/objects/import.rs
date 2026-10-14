use strum::IntoEnumIterator;

/// The `Display` strings are the usernames of the bots, without the `@`.
#[derive(Debug, PartialEq, Eq, Clone, Copy, strum_macros::Display, strum_macros::EnumIter)]
pub enum OriginalBotKind {
    #[strum(serialize = "pipisabot")]
    Pipisa,
    #[strum(serialize = "kraft28_bot")]
    Kraft28,
}

impl OriginalBotKind {
    /// Whether a leaderboard of this bot, which shows `shown_name`, may mean a user whose name is
    /// `full_name`. Pipisa cuts the name short; Kraft28 shows the first name only.
    pub fn matches_name(&self, full_name: &str, shown_name: &str) -> bool {
        match self {
            OriginalBotKind::Pipisa => full_name.chars().take(13).eq(shown_name.chars()),
            OriginalBotKind::Kraft28 => {
                full_name == shown_name
                    || full_name.strip_prefix(shown_name).is_some_and(|rest| rest.starts_with(' '))
            }
        }
    }
}

impl TryFrom<&str> for OriginalBotKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let username = value.trim_start_matches('@');
        Self::iter()
            .find(|kind| kind.to_string() == username)
            .ok_or_else(|| "Unknown OriginalBotKind".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipisa_shows_the_first_thirteen_characters() {
        let p = OriginalBotKind::Pipisa;
        assert!(p.matches_name("SadBot #incel>suicide", "SadBot #incel"));
        assert!(p.matches_name("SadBot", "SadBot"));
        assert!(!p.matches_name("SadBot #incel>suicide", "SadBot #inc"));
    }

    #[test]
    fn kraft28_shows_the_first_name() {
        let k = OriginalBotKind::Kraft28;
        assert!(k.matches_name("Leonid SadBot Rust'a'man", "Leonid"));
        assert!(k.matches_name("Leonid", "Leonid"));
        assert!(k.matches_name("Mary Ann Smith", "Mary Ann"));
        assert!(!k.matches_name("Leonidas", "Leonid"));
        assert!(!k.matches_name("Leonid SadBot", "SadBot"));
    }

    #[test]
    fn original_bot_kind_try_from() {
        let check = |variant: &str, kind| {
            let second_variant = variant.strip_prefix('@').expect("no '@' prefix");
            let valid_variants = [variant, second_variant];
            assert!(valid_variants.into_iter()
                .all(|v| OriginalBotKind::try_from(v)
                    .is_ok_and(|k| k == kind)));

            let invalid_variants = valid_variants.into_iter()
                .map(|v| {
                    v.strip_suffix("_bot")
                        .or_else (|| v.strip_suffix("bot"))
                        .expect("no 'bot' suffix")
                });
            assert!(invalid_variants.into_iter()
                .all(|v| OriginalBotKind::try_from(v)
                    .is_err()));
        } ;

        check("@pipisabot", OriginalBotKind::Pipisa);
        check("@kraft28_bot", OriginalBotKind::Kraft28);
    }
}
