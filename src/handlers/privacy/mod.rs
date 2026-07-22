use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::config::MessageGroup;
use crate::domain::primitives::LanguageCode;
use crate::domain::primitives::SupportedLanguage::{EN, RU, IT, FA, ZH};
use crate::handlers::{HandlerResult, reply_html};
use crate::handlers::utils::SelfDestructionService;
use crate::{metrics, reply_html_ephemeral};

static EN_POLICY: &str = include_str!("en.html");
static RU_POLICY: &str = include_str!("ru.html");
static IT_POLICY: &str = include_str!("it.html");
static FA_POLICY: &str = include_str!("fa.html");
static ZH_POLICY: &str = include_str!("zh.html");

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum PrivacyCommands {
    #[command(description = "privacy")]
    Privacy,
}

pub async fn privacy_cmd_handler(bot: Bot, msg: Message,
                                 self_destruction: SelfDestructionService) -> HandlerResult {
    metrics::CMD_PRIVACY_COUNTER.inc();
    let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
    let policy = match lang_code.to_supported_language() {
        RU => RU_POLICY,
        EN => EN_POLICY,
        IT => IT_POLICY,
        FA => FA_POLICY,
        ZH => ZH_POLICY,
    };
    reply_html_ephemeral!(bot, msg, policy, self_destruction, MessageGroup::Notice);
    Ok(())
}
