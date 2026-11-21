use autometrics::autometrics;
use reqwest::Url;
use rust_i18n::t;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use teloxide::types::ReplyMarkup;
use crate::commands::{CommandToggles, MenuScope, PublishedCommands};
use crate::config::{AppConfig, ThrottledBot};
use crate::config::MessageGroup::Notice;
use crate::domain::primitives::{LanguageCode, StartParameter, Username};
use crate::domain::primitives::SupportedLanguage::{EN, RU, IT, FA, ZH};
use crate::errors::flat::flat;
use crate::handlers::{HandlerDeps, HandlerImplResult, HandlerResult, LinkButton, reply_html};
use crate::{metrics, reply_html_ephemeral};

static VOUCHER: Embedded = Embedded {
    en: include_str!("voucher/en.html"),
    ru: include_str!("voucher/ru.html"),
    it: include_str!("voucher/it.html"),
    fa: include_str!("voucher/fa.html"),
    zh: include_str!("voucher/zh.html"),
};

static RESET: Embedded = Embedded {
    en: include_str!("reset/en.html"),
    ru: include_str!("reset/ru.html"),
    it: include_str!("reset/it.html"),
    fa: include_str!("reset/fa.html"),
    zh: include_str!("reset/zh.html"),
};

/// Commands that answer with a fixed text and, optionally, a link button. The variants without a
/// description are aliases: they stay out of the menu and are commands like any other everywhere else.
#[derive(BotCommands, Clone, Copy, Debug, PartialEq)]
#[command(rename_rule = "lowercase")]
pub enum InfoCommands {
    #[command(description = "voucher")]
    Voucher,
    Send,
    Transfer,
    Giveaway,
    #[command(description = "reset")]
    Reset,
    #[command(description = "buy")]
    Buy,
    Auctions,
    #[command(description = "donate")]
    Donate,
}

impl InfoCommands {
    /// The name of the main command, which its aliases share: the translation keys, the
    /// `DISABLE_CMD_*` switch and the counter go by it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Voucher | Self::Send | Self::Transfer | Self::Giveaway => "voucher",
            Self::Reset => "reset",
            Self::Buy | Self::Auctions => "buy",
            Self::Donate => "donate",
        }
    }
}

impl PublishedCommands for InfoCommands {
    /// `/voucher` and `/reset` are about a chat's leaderboard, so they are in the group menus only,
    /// and resetting one is for its administrators. `/buy` needs an auction bot to point to, and
    /// `/donate` a page. The administrators' menu adds to the group one, so nothing but `/reset` is
    /// repeated there.
    fn is_published(&self, scope: MenuScope, toggles: &CommandToggles) -> bool {
        match self {
            Self::Voucher | Self::Send | Self::Transfer | Self::Giveaway => scope == MenuScope::Group,
            Self::Reset => scope == MenuScope::Admins,
            Self::Buy | Self::Auctions => scope != MenuScope::Admins && toggles.auction_enabled,
            Self::Donate => scope != MenuScope::Admins && toggles.donate_enabled,
        }
    }
}

pub fn is_enabled(cmd: InfoCommands, config: AppConfig) -> bool {
    config.command_toggles.enabled(cmd.key())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn info_cmd_handler(
    bot: ThrottledBot,
    msg: Message,
    cmd: InfoCommands,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { config, self_destruction, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let response = info_impl(cmd, &config, &lang_code);
    let keyboard = response.keyboard().map(ReplyMarkup::InlineKeyboard);
    reply_html_ephemeral!(bot, msg, response.text(), self_destruction, Notice, lang_code,
        reply_markup = keyboard);
    Ok(())
}

/// Counts the invocation and builds the answer to it, for a chat and for inline mode alike.
pub(crate) fn info_impl(cmd: InfoCommands, config: &AppConfig, lang_code: &LanguageCode) -> HandlerImplResult {
    count(cmd);
    build(cmd, config, lang_code)
}

struct Embedded {
    en: &'static str,
    ru: &'static str,
    it: &'static str,
    fa: &'static str,
    zh: &'static str,
}

impl Embedded {
    fn get(&self, lang_code: &LanguageCode) -> &'static str {
        match lang_code.to_supported_language() {
            RU => self.ru,
            EN => self.en,
            IT => self.it,
            FA => self.fa,
            ZH => self.zh,
        }
    }
}

fn count(cmd: InfoCommands) {
    let counter = match cmd {
        InfoCommands::Voucher | InfoCommands::Send | InfoCommands::Transfer | InfoCommands::Giveaway => &metrics::CMD_VOUCHER_COUNTER,
        InfoCommands::Reset => &metrics::CMD_RESET_COUNTER,
        InfoCommands::Buy | InfoCommands::Auctions => &metrics::CMD_BUY_COUNTER,
        InfoCommands::Donate => &metrics::CMD_DONATE_COUNTER,
    };
    counter.inc();
}

/// A link that isn't configured leaves its command without a button.
fn build(cmd: InfoCommands, config: &AppConfig, lang_code: &LanguageCode) -> HandlerImplResult {
    let key = cmd.key();
    match cmd {
        InfoCommands::Voucher | InfoCommands::Send | InfoCommands::Transfer | InfoCommands::Giveaway =>
            HandlerImplResult::OnlyText(VOUCHER.get(lang_code).to_owned()),
        InfoCommands::Reset =>
            HandlerImplResult::OnlyText(RESET.get(lang_code).to_owned()),
        InfoCommands::Buy | InfoCommands::Auctions => match &config.auction_bot {
            Some(auction_bot) => {
                let text = t!("commands.buy.text", bot = auction_bot, locale = lang_code).into_owned();
                if let Some(url) = auction_link(auction_bot, config.auction_start_parameter.as_ref()) {
                    HandlerImplResult::WithLink { text, button: link_button(key, url, lang_code) }
                } else {
                    HandlerImplResult::OnlyText(text)
                }
            },
            None => feature_disabled(lang_code),
        },
        InfoCommands::Donate => match &config.donate_url {
            Some(donate_url) => HandlerImplResult::WithLink {
                text: t!("commands.donate.text", locale = lang_code).into_owned(),
                button: link_button(key, donate_url.clone(), lang_code),
            },
            None => feature_disabled(lang_code),
        },
    }
}

fn feature_disabled(lang_code: &LanguageCode) -> HandlerImplResult {
    HandlerImplResult::OnlyText(t!("errors.feature_disabled", locale = lang_code).into_owned())
}

fn auction_link(auction_bot: &Username, start_parameter: Option<&StartParameter>) -> Option<Url> {
    let name = auction_bot.value().trim_start_matches('@');
    let url = match start_parameter {
        Some(start_parameter) => format!("https://t.me/{name}?start={start_parameter}"),
        None => format!("https://t.me/{name}"),
    };
    Url::parse(&url)
        .inspect_err(|e| tracing::warn!(error = flat!(e), %url, "couldn't parse the link of an info button"))
        .ok()
}

fn link_button(key: &str, url: Url, lang_code: &LanguageCode) -> LinkButton {
    let label_key = format!("commands.{key}.button");
    let label = t!(&label_key, locale = lang_code).into_owned();
    LinkButton::new(label, url)
}

#[cfg(test)]
mod test {
    use rust_i18n::t;
    use teloxide::types::InlineKeyboardButtonKind;
    use crate::domain::primitives::{LanguageCode, StartParameter, Username};
    use domain_types::literal;
    use reqwest::Url;
    use crate::config::AppConfig;
    use crate::handlers::HandlerImplResult;
    use super::{build, InfoCommands, RESET, VOUCHER};

    const LANGUAGES: [&str; 6] = ["en", "ru", "it", "fa", "zh", "zh-TW"];

    #[test]
    fn aliases_share_their_main_command() {
        let pairs = [
            (InfoCommands::Send, "voucher"),
            (InfoCommands::Transfer, "voucher"),
            (InfoCommands::Giveaway, "voucher"),
            (InfoCommands::Auctions, "buy"),
        ];
        for (alias, key) in pairs {
            assert_eq!(alias.key(), key, "{alias:?}");
        }
    }

    #[test]
    fn embedded_texts_exist_in_every_language() {
        for lang in LANGUAGES {
            let lang_code = language(lang);
            assert!(!VOUCHER.get(&lang_code).trim().is_empty(), "voucher, {lang}");
            assert!(!RESET.get(&lang_code).trim().is_empty(), "reset, {lang}");
        }
    }

    #[test]
    fn translations_exist_in_every_language() {
        let keys = [
            "commands.voucher.description",
            "commands.reset.description",
            "commands.buy.description",
            "commands.buy.text",
            "commands.buy.button",
            "commands.donate.description",
            "commands.donate.text",
            "commands.donate.button",
        ];
        for lang in LANGUAGES {
            for key in keys {
                let translated = t!(key, locale = lang);
                assert_ne!(translated, key, "{lang}");
            }
        }
    }

    #[test]
    fn buy_links_to_the_auction_bot_when_there_is_one() {
        let lang_code = language("en");

        let tagged = AppConfig {
            auction_bot: Some(Username::from("DickAuctionBot")),
            auction_start_parameter: Some(literal!(StartParameter = "utm-dgb")),
            ..AppConfig::default()
        };
        let with = build(InfoCommands::Auctions, &tagged, &lang_code);
        let text = with.text();
        assert!(text.contains("@DickAuctionBot"), "{text}");
        let url = button_url(&with);
        assert_eq!(url.as_deref(), Some("https://t.me/DickAuctionBot?start=utm-dgb"));

        let bare = AppConfig { auction_start_parameter: None, ..tagged };
        let untagged = build(InfoCommands::Buy, &bare, &lang_code);
        let url = button_url(&untagged);
        assert_eq!(url.as_deref(), Some("https://t.me/DickAuctionBot"));

        let without = build(InfoCommands::Buy, &AppConfig::default(), &lang_code);
        assert!(without.keyboard().is_none());
        let text = without.text();
        assert!(!text.contains("DickAuctionBot"), "{text}");
    }

    #[test]
    fn donate_links_to_the_donation_page_when_there_is_one() {
        let donate_url = Url::parse("https://example.org/donate?utm_source=test")
            .expect("couldn't parse the test URL");
        let config = AppConfig { donate_url: Some(donate_url), ..AppConfig::default() };
        for lang in LANGUAGES {
            let response = build(InfoCommands::Donate, &config, &language(lang));
            let url = button_url(&response);
            assert_eq!(url.as_deref(), Some("https://example.org/donate?utm_source=test"), "{lang}");
        }

        let without = build(InfoCommands::Donate, &AppConfig::default(), &language("en"));
        assert!(without.keyboard().is_none());
    }

    fn language(lang: &str) -> LanguageCode {
        LanguageCode::new(lang.to_owned())
    }

    fn button_url(response: &HandlerImplResult) -> Option<String> {
        response.keyboard()
            .and_then(|keyboard| keyboard.inline_keyboard.into_iter().flatten().next())
            .and_then(|button| match button.kind {
                InlineKeyboardButtonKind::Url(url) => Some(url.to_string()),
                _ => None,
            })
    }
}
