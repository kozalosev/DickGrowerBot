use std::fmt::{Display, Formatter};
use std::sync::Arc;
use itertools::Itertools;
use teloxide::utils::command::BotCommands;
use crate::domain::primitives::Username;

/// Every username the bot answers to, the one `getMe` returns first.
#[derive(Clone, Debug)]
pub struct BotNames(Arc<[Username]>);

impl BotNames {
    pub fn new(primary: Username, others: impl IntoIterator<Item = Username>) -> Self {
        let names = std::iter::once(primary)
            .chain(others)
            .unique_by(|name| name.value().to_ascii_lowercase())
            .collect();
        Self(names)
    }

    pub fn is_ours(&self, name: &str) -> bool {
        self.0.iter().any(|known| known.value().eq_ignore_ascii_case(name))
    }

    /// The command, if the text is one addressed to nobody or to one of our names.
    pub fn parse<C: BotCommands>(&self, text: &str) -> Option<C> {
        let addressee = text.split_whitespace()
            .next()
            .and_then(|word| word.split_once('@'))
            .map(|(_, name)| name)
            .filter(|name| self.is_ours(name));
        C::parse(text, addressee.unwrap_or_else(|| self.primary())).ok()
    }

    fn primary(&self) -> &str {
        self.0[0].value()
    }
}

impl Display for BotNames {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let names = self.0.iter()
            .map(Username::value_with_at_sign)
            .join(", ");
        f.write_str(&names)
    }
}

#[cfg(test)]
mod test {
    use teloxide::macros::BotCommands;
    use crate::domain::primitives::Username;
    use super::BotNames;

    #[derive(BotCommands, Debug, PartialEq)]
    #[command(rename_rule = "lowercase")]
    enum Command {
        Grow,
        Pvp(String),
    }

    #[test]
    fn a_bare_command_is_ours() {
        assert_eq!(names().parse("/grow"), Some(Command::Grow));
        assert_eq!(names().parse("/pvp 10"), Some(Command::Pvp("10".to_owned())));
    }

    #[test]
    fn a_command_to_any_of_our_names_is_ours_in_any_case() {
        assert_eq!(names().parse("/grow@DickGrowerBot"), Some(Command::Grow));
        assert_eq!(names().parse("/grow@dickgrowergame"), Some(Command::Grow));
        assert_eq!(names().parse("/pvp@mydick 10"), Some(Command::Pvp("10".to_owned())));
    }

    #[test]
    fn a_command_to_another_bot_is_not_ours() {
        assert_eq!(names().parse::<Command>("/grow@SomeOtherBot"), None);
        assert_eq!(names().parse::<Command>("/grow@"), None);
    }

    #[test]
    fn only_the_primary_name_is_known_without_others() {
        let names = BotNames::new(Username::from("DickGrowerBot"), []);
        assert_eq!(names.parse("/grow@DickGrowerBot"), Some(Command::Grow));
        assert_eq!(names.parse::<Command>("/grow@DickGrowerGame"), None);
    }

    #[test]
    fn the_names_are_deduplicated_regardless_of_case() {
        let names = BotNames::new(
            Username::from("DickGrowerBot"),
            ["dickgrowerbot", "DickGrowerGame"].map(Username::from),
        );
        assert_eq!(names.to_string(), "@DickGrowerBot, @DickGrowerGame");
    }

    fn names() -> BotNames {
        BotNames::new(
            Username::from("DickGrowerBot"),
            ["DickGrowerBot", "DickGrowerGame", "mydick"].map(Username::from),
        )
    }
}
