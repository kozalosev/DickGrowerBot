use strum::IntoEnumIterator;

const PIPISA_NAME_LENGTH: usize = 13;
const ELLIPSIS: &str = "...";
const PIPISA_MARKS: [&str; 2] = ["🍆", "🤥"];

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
    /// `full_name`.
    ///
    /// Pipisa shows a name of up to 13 characters whole. A longer one is cut to 13 characters and
    /// gets the ellipsis. The first place of a leaderboard is marked with 🍆 and the last one with 🤥,
    /// whatever the length of the name; the mark goes before the ellipsis. Kraft28 shows the first
    /// name only.
    pub fn matches_name(&self, full_name: &str, shown_name: &str) -> bool {
        match self {
            OriginalBotKind::Pipisa => {
                full_name == shown_name
                    || full_name == strip_mark(shown_name)
                    || shown_name.strip_suffix(ELLIPSIS).is_some_and(|cut| {
                        full_name.chars().count() > PIPISA_NAME_LENGTH
                            && full_name.chars().take(PIPISA_NAME_LENGTH).eq(strip_mark(cut).chars())
                    })
            },
            OriginalBotKind::Kraft28 => {
                full_name == shown_name
                    || full_name.strip_prefix(shown_name).is_some_and(|rest| rest.starts_with(' '))
            }
        }
    }
}

fn strip_mark(name: &str) -> &str {
    PIPISA_MARKS.iter()
        .find_map(|mark| name.strip_suffix(mark))
        .unwrap_or(name)
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
    fn pipisa_shows_a_short_name_whole() {
        let p = OriginalBotKind::Pipisa;
        assert!(p.matches_name("SadBot", "SadBot"));
        assert!(p.matches_name("Alice...", "Alice..."));
        assert!(!p.matches_name("SadBot #incel", "SadBot"));
        assert!(!p.matches_name("Alice", "Bob"));
    }

    #[test]
    fn pipisa_cuts_a_long_name_to_thirteen_characters() {
        let p = OriginalBotKind::Pipisa;
        assert!(p.matches_name("Leonid SadBot Rust'a'man", "Leonid SadBot🍆..."));
        assert!(p.matches_name("Leonid Kozarin", "Leonid Kozari🤥..."));
        assert!(p.matches_name("Leonid Kozarin", "Leonid Kozari..."));
        assert!(p.matches_name("Hello, world!!", "Hello, world!🍆..."));
    }

    #[test]
    fn pipisa_marks_a_short_name_of_the_first_and_the_last_place() {
        let p = OriginalBotKind::Pipisa;
        assert!(p.matches_name("Кошка", "Кошка🤥"));
        assert!(p.matches_name("Кошка", "Кошка🍆"));
        assert!(!p.matches_name("Кошка", "Кошка🐱"), "not one of the two marks");
        assert!(!p.matches_name("Собака", "Кошка🤥"));
    }

    #[test]
    fn pipisa_requires_the_cut_and_the_ellipsis() {
        let p = OriginalBotKind::Pipisa;
        assert!(!p.matches_name("Leonid Kozarin", "Leonid Kozarix..."), "a letter instead of a mark");
        assert!(!p.matches_name("Leonid Kozarin", "Leonid Kozari"), "no ellipsis, a name of its own");
        assert!(!p.matches_name("Leonid Kozarin", "Leonid Kozari🤥"), "no ellipsis");
        assert!(!p.matches_name("Leonid Kozarin", "Leonid Koza🤥..."), "cut at the wrong place");
        assert!(!p.matches_name("Leonid Kozari", "Leonid Kozari🤥..."), "was short enough to be shown whole");
        assert!(!p.matches_name("Alice", "Bob🍆..."));
    }

    #[test]
    fn pipisa_counts_characters_whatever_is_inside_the_name() {
        let p = OriginalBotKind::Pipisa;
        assert!(p.matches_name("Le🍆o Kozarin Smith", "Le🍆o Kozarin 🤥..."));
        assert!(p.matches_name(
            "👨\u{200d}👩\u{200d}👧 Family Name",
            "👨\u{200d}👩\u{200d}👧 Family 🤥...",
        ));
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
