use std::fmt::Display;
use std::str::FromStr;
use linkify::{LinkFinder, LinkKind};
use once_cell::sync::Lazy;
use regex::{NoExpand, Regex};
use unicode_general_category::GeneralCategory::Format;
use unicode_general_category::get_general_category;
use domain_types::errors::EmptyStringValue;
use domain_types_macro::domain_type;

const LTR_MARK: char = '\u{200E}';
const BLOCKED_LINK: &str = r#"<tg-emoji emoji-id="5400336550498157996">🚫</tg-emoji>"#;

/// The domains a link without a scheme is hidden in. `linkify` checks a TLD only for its letters, so
/// without this list any name with a dot in it would be taken for a link.
const KNOWN_TLDS: [&str; 42] = [
    "com", "net", "org", "ru", "su", "ua", "by", "kz", "me", "dog", "io", "co", "xyz", "top",
    "site", "online", "shop", "store", "club", "info", "biz", "pro", "app", "dev", "link", "live",
    "fun", "ly", "gg", "to", "cc", "tk", "ml", "ga", "cf", "gq", "in", "us", "uk", "eu", "de", "sh",
];

/// The non-ASCII TLDs, which `linkify` never takes for a domain without a scheme.
const KNOWN_IDN_TLDS: [&str; 7] = ["рф", "рус", "укр", "бел", "срб", "қаз", "мкд"];

static IDN_LINK_REGEX: Lazy<Regex> = Lazy::new(|| {
    let tlds = KNOWN_IDN_TLDS.join("|");
    Regex::new(&format!(r"(?i)(?:[\p{{L}}\p{{N}}-]+\.)+(?:{tlds})\b(?:[/?#]\S*)?"))
        .expect("invalid regex for links with a non-ASCII TLD")
});

static LINK_FINDER: Lazy<LinkFinder> = Lazy::new(|| {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    finder.url_must_have_scheme(false);
    finder
});

#[domain_type(
    features(no_auto_display)
)]
pub struct Username(String);

impl Username {
    /// The name as HTML, with every link in it replaced by an emoji, so that Telegram can't make
    /// it clickable.
    pub fn escaped(&self) -> String {
        let safe_name: String = self.value().chars()
            .filter(|c| get_general_category(*c) != Format)
            .collect();
        let html: String = LINK_FINDER.spans(&safe_name)
            .map(|span| match span.kind() {
                Some(LinkKind::Url) if is_link_to_hide(span.as_str()) => BLOCKED_LINK.to_owned(),
                Some(_) => teloxide::utils::html::escape(span.as_str()),
                None => {
                    let escaped = teloxide::utils::html::escape(span.as_str());
                    IDN_LINK_REGEX.replace_all(&escaped, NoExpand(BLOCKED_LINK)).into_owned()
                },
            })
            .collect();
        format!("{LTR_MARK}{html}{LTR_MARK}")
    }

    pub fn value_with_at_sign(&self) -> String {
        if self.0.starts_with('@') {
            self.0.clone()
        } else {
            format!("@{}", self.0)
        }
    }
}

impl From<&str> for Username {
    fn from(value: &str) -> Self {
        Username::new(value.to_owned())
    }
}

impl FromStr for Username {
    type Err = EmptyStringValue;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            Err(EmptyStringValue)
        } else {
            Ok(Username::from(s))
        }
    }
}

impl Display for Username {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value_with_at_sign())
    }
}

fn is_link_to_hide(link: &str) -> bool {
    let contains_scheme = link.contains("://");
    let check_tld_is_known = || link
        .split(['/', '?', '#', ':'])
        .next()
        .and_then(|host| host.trim_end_matches('.').rsplit('.').next())
        .is_some_and(|tld| KNOWN_TLDS.contains(&tld.to_lowercase().as_str()));

    contains_scheme || check_tld_is_known()
}

#[cfg(test)]
mod test {
    use crate::domain::primitives::Username;
    use super::{BLOCKED_LINK, LTR_MARK};

    #[test]
    fn test_username_escaped() {
        let username = Username::from("<3 Tom & Jerry >_<");
        assert_eq!(username.escaped(), "\u{200E}&lt;3 Tom &amp; Jerry &gt;_&lt;\u{200E}");
    }

    #[test]
    fn test_username_value_with_at_sign() {
        let result = "@test";
        for variant in ["test", "@test"] {
            let username = Username::from(variant);
            assert_eq!(username.value_with_at_sign(), result);
        }
    }

    #[test]
    fn links_with_a_scheme_are_hidden() {
        let username = Username::from("Free & https://scam.example/x?a=1 today");
        assert_eq!(username.escaped(), ltr(&format!("Free &amp; {BLOCKED_LINK} today")));
    }

    #[test]
    fn links_to_known_domains_are_hidden() {
        let cases = [
            ("Vasya t.me/spam_bot", format!("Vasya {BLOCKED_LINK}")),
            ("buy at Shop.RU!", format!("buy at {BLOCKED_LINK}!")),
            ("сайт.рф", BLOCKED_LINK.to_owned()),
            ("Vasya пример.РУС/x?a&b", format!("Vasya {BLOCKED_LINK}")),
            ("a.b t.me/x c.рф", format!("a.b {BLOCKED_LINK} {BLOCKED_LINK}")),
            ("Join t.me/x now & win", format!("Join {BLOCKED_LINK} now &amp; win")),
            ("Заходи на сайт.рф/игра и <жми>", format!("Заходи на {BLOCKED_LINK} и &lt;жми&gt;")),
            ("Заходи на сайт.рф, жми", format!("Заходи на {BLOCKED_LINK}, жми")),
        ];
        for (name, expected) in cases {
            let username = Username::from(name);
            assert_eq!(username.escaped(), ltr(&expected), "{name}");
        }
    }

    #[test]
    fn names_with_dots_are_left_alone() {
        for name in ["Mr.Smith", "J.R.R. Tolkien", "v1.2.3", "file.txt", "Иван.Рфович"] {
            let username = Username::from(name);
            assert_eq!(username.escaped(), ltr(name), "{name}");
        }
    }

    #[test]
    fn invisible_characters_do_not_hide_a_link() {
        let username = Username::from("t\u{200B}.me/x");
        assert_eq!(username.escaped(), ltr(BLOCKED_LINK));
    }

    #[test]
    fn the_rest_of_the_name_is_still_escaped() {
        let username = Username::from("<b>x</b> t.me/x");
        assert_eq!(username.escaped(), ltr(&format!("&lt;b&gt;x&lt;/b&gt; {BLOCKED_LINK}")));
    }

    fn ltr(s: &str) -> String {
        format!("{LTR_MARK}{s}{LTR_MARK}")
    }
}
