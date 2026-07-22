use rust_i18n::t;
use serde::Serialize;
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

#[derive(Serialize, Clone)]
pub struct Context {
    pub bot_name: Username,
    pub grow_min: String,
    pub grow_max: String,
    pub other_bots: String,
    pub admin_channel_ru: Username,
    pub admin_channel_en: Username,
    pub admin_chat_ru: Username,
    pub admin_chat_en: Username,
    pub git_repo: String,
    pub help_pussies_percentage: Percentage,
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
