use rust_i18n::t;
use serde::Serialize;
use tinytemplate::TinyTemplate;

static EN_HELP: &str = include_str!("en.md");
static RU_HELP: &str = include_str!("ru.md");

#[derive(Clone)]
pub struct HelpContainer {
    en: String,
    ru: String,
}

impl HelpContainer {
    pub fn get_start_message(&self, username: String, lang_code: String) -> String {
        let greeting = t!("titles.greeting", locale = &lang_code);
        format!("{}, <b>{}</b>!\n\n{}", greeting, username, self.get_help_message(lang_code))
    }

    pub fn get_help_message(&self, lang_code: String) -> String {
        match lang_code.as_str() {
            "ru" => self.ru.clone(),
            _ => self.en.clone()
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
