use rust_i18n::t;
use serde::Serialize;
use tinytemplate::TinyTemplate;
use crate::domain::SupportedLanguage::{EN, RU};
use crate::domain::{LanguageCode, Username};

static EN_HELP: &str = include_str!("en.html");
static RU_HELP: &str = include_str!("ru.html");

#[derive(Clone)]
pub struct HelpContainer {
    en: String,
    ru: String,
}

impl HelpContainer {
    pub fn get_start_message(&self, username: Username, lang_code: LanguageCode) -> String {
        let greeting = t!("titles.greeting", locale = &lang_code);
        format!("{}, <b>{}</b>!\n\n{}", greeting, username.escaped(), self.get_help_message(lang_code))
    }

    pub fn get_help_message(&self, lang_code: LanguageCode) -> String {
        match lang_code.to_supported_language() {
            RU => self.ru.clone(),
            EN => self.en.clone()
        }
    }
}

#[derive(Serialize, Clone)]
pub struct Context {
    pub bot_name: String,
    pub grow_min: String,
    pub grow_max: String,
    pub other_bots: String,
    pub admin_username: String,
    pub admin_channel_ru: String,
    pub admin_channel_en: String,
    pub git_repo: String,
    pub help_pussies_percentage: f64
}

pub fn render_help_messages(context: Context) -> Result<HelpContainer, tinytemplate::error::Error> {
    let mut tt = TinyTemplate::new();
    tt.add_template("en", EN_HELP)?;
    tt.add_template("ru", RU_HELP)?;
    Ok(HelpContainer {
        en: tt.render("en", &context)?,
        ru: tt.render("ru", &context)?,
    })
}
