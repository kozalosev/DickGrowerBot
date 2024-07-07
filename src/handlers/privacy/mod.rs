use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::metrics;

static EN_POLICY: &str = include_str!("en.html");
static RU_POLICY: &str = include_str!("ru.html");

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum PrivacyCommands {
    #[command(description = "privacy")]
    Privacy,
}

pub async fn privacy_cmd_handler(bot: Bot, msg: Message) -> HandlerResult {
    metrics::CMD_PRIVACY_COUNTER.inc();
    let lang_code = ensure_lang_code(msg.from());
    let policy = match lang_code.as_str() {
        "ru" => RU_POLICY,
        _    => EN_POLICY,
    };
    reply_html(bot, msg, policy).await?;
    Ok(())
}
