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
    pub fn convert_name(&self, name: &str) -> String {
        match self {
            OriginalBotKind::Pipisa => {
                name.chars()
                    .take(13)
                    .collect()
            },
            OriginalBotKind::Kraft28 => name.to_owned()
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
    fn original_bot_kind_convert_name_pipisa() {
        let (p, short) = (OriginalBotKind::Pipisa, "SadBot #incel".to_owned());
        assert_eq!(p.convert_name("SadBot #incel..."), short);
        assert_eq!(p.convert_name("SadBot #incel>suicide"), short);
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
