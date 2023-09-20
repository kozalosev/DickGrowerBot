use rust_i18n::t;
use teloxide::prelude::UserId;
use teloxide::types::{Me, User};

static EN_HELP: &str = include_str!("en.md");
static RU_HELP: &str = include_str!("ru.md");

pub fn get_start_message(from: &User, me: Me) -> String {
    let lang_code = &ensure_lang_code(from.id, from.language_code.clone());
    let greeting = t!("title.greeting", locale = lang_code);
    format!("{}, *{}*\\!\n\n{}", greeting, from.first_name, get_help_message(Some(from), me))
}

pub fn get_help_message(from: Option<&User>, me: Me) -> String {
    let help_template = from.and_then(|u| u.language_code.clone())
        .filter(|lang_code| lang_code == "ru")
        .map(|_| RU_HELP)
        .unwrap_or(EN_HELP);
    help_template.replace("{{bot_name}}", me.username())
}

fn ensure_lang_code(uid: UserId, lang_code: Option<String>) -> String {
    lang_code
        .unwrap_or_else(|| {
            log::warn!("no language_code for {}, using the default", uid);
            String::default()
        })
}