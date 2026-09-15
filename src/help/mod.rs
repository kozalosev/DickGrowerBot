use rust_i18n::t;
use serde::Serialize;
use serde_with::{serde_as, DisplayFromStr};
use tinytemplate::TinyTemplate;
use crate::domain::primitives::{LanguageCode, Percentage, Username};
use crate::domain::primitives::SupportedLanguage::{EN, RU, IT, FA, ZH};

static EN_HELP: &str = include_str!("en.html");
static RU_HELP: &str = include_str!("ru.html");
static IT_HELP: &str = include_str!("it.html");
static FA_HELP: &str = include_str!("fa.html");
static ZH_HELP: &str = include_str!("zh.html");

#[derive(Clone)]
pub struct HelpContainer {
    en: String,
    ru: String,
    it: String,
    fa: String,
    zh: String,
}

impl HelpContainer {
    pub fn get_start_message(&self, username: Username, lang_code: LanguageCode) -> String {
        let greeting = t!("titles.greeting", locale = &lang_code);
        format!("{}, <b>{}</b>!\n\n{}", greeting, username.escaped(), self.get_help_message(&lang_code))
    }

    pub fn get_help_message(&self, lang_code: &LanguageCode) -> String {
        match lang_code.to_supported_language() {
            RU => self.ru.clone(),
            EN => self.en.clone(),
            IT => self.it.clone(),
            FA => self.fa.clone(),
            ZH => self.zh.clone(),
        }
    }
}

#[serde_as]
#[derive(Serialize, Clone)]
pub struct Context {
    pub bot_name: Username,
    pub grow_min: String,
    pub grow_max: String,
    pub other_bots: String,
    #[serde_as(as = "DisplayFromStr")]
    pub admin_channel_ru: Username,
    #[serde_as(as = "DisplayFromStr")]
    pub admin_channel_en: Username,
    #[serde_as(as = "DisplayFromStr")]
    pub admin_chat_ru: Username,
    #[serde_as(as = "DisplayFromStr")]
    pub admin_chat_en: Username,
    pub git_repo: String,
    pub support_enabled: bool,
    pub cleanup_enabled: bool,
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub auction_bot: Option<Username>,
    pub help_pussies_percentage: Percentage,
    pub streak_bonus_grades: String,
}

pub fn render_help_messages(context: Context) -> Result<HelpContainer, tinytemplate::error::Error> {
    let mut tt = TinyTemplate::new();
    tt.add_template("en", EN_HELP)?;
    tt.add_template("ru", RU_HELP)?;
    tt.add_template("it", IT_HELP)?;
    tt.add_template("fa", FA_HELP)?;
    tt.add_template("zh", ZH_HELP)?;
    Ok(HelpContainer {
        en: tt.render("en", &context)?,
        ru: tt.render("ru", &context)?,
        it: tt.render("it", &context)?,
        fa: tt.render("fa", &context)?,
        zh: tt.render("zh", &context)?,
    })
}

#[cfg(test)]
mod test {
    use domain_types::literal;
    use crate::domain::primitives::{LanguageCode, Percentage, Username};
    use crate::help::{render_help_messages, Context};

    fn context() -> Context {
        Context {
            bot_name: Username::from("DickGrowerBot"),
            grow_min: "-5".to_owned(),
            grow_max: "10".to_owned(),
            other_bots: String::new(),
            admin_channel_ru: Username::from("kozaloru"),
            admin_channel_en: Username::from("@kozalo_blog"),
            admin_chat_ru: Username::from("kozalo_chat_ru"),
            admin_chat_en: Username::from("kozalo_chat_en"),
            git_repo: String::new(),
            support_enabled: true,
            cleanup_enabled: true,
            auction_bot: Some(Username::from("DickAuctionBot")),
            help_pussies_percentage: literal!(Percentage = 0),
            streak_bonus_grades: "2, 4, 8, 16, 32".to_owned(),
        }
    }

    #[test]
    fn channels_and_chats_are_rendered_with_an_at_sign() {
        let help = render_help_messages(context())
            .expect("the help must render");

        let ru = help.get_help_message(&LanguageCode::new("ru".to_owned()));
        assert!(ru.contains("канал @kozaloru,"), "{ru}");
        assert!(ru.contains("пишите @kozalo_chat_ru,"), "{ru}");

        let en = help.get_help_message(&LanguageCode::new("en".to_owned()));
        assert!(en.contains("Subscribe to @kozalo_blog "), "an at sign already there must not be doubled: {en}");
        assert!(en.contains("write to @kozalo_chat_en,"), "{en}");
    }

    #[test]
    fn optional_features_are_mentioned_only_when_they_are_there() {
        let shown = render_help_messages(context())
            .expect("the help must render");
        let hidden = render_help_messages(Context {
            support_enabled: false,
            cleanup_enabled: false,
            auction_bot: None,
            streak_bonus_grades: String::new(),
            ..context()
        }).expect("the help must render");

        for lang in ["en", "ru", "it", "fa", "zh"] {
            let lang_code = LanguageCode::new(lang.to_owned());
            let without = hidden.get_help_message(&lang_code);
            assert!(!without.contains("/support"), "{lang}: {without}");
            assert!(!without.contains("/promo"), "{lang}: {without}");
            assert!(!without.contains("/cleanup"), "{lang}: {without}");
            assert!(!without.contains("<b></b>"), "{lang}: {without}");

            let with = shown.get_help_message(&lang_code);
            assert!(with.contains("/support"), "{lang}: {with}");
            assert!(with.contains("/cleanup"), "{lang}: {with}");
            assert!(with.contains("<b>2, 4, 8, 16, 32</b>"), "{lang}: {with}");
            assert!(with.contains("@DickAuctionBot"), "{lang}: {with}");
            assert!(with.contains("/promo"), "{lang}: {with}");
        }
    }
}
