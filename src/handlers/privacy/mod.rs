use autometrics::autometrics;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::config::MessageGroup::Notice;
use crate::domain::primitives::SupportedLanguage::{EN, RU, IT, FA, ZH};
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
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

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn privacy_cmd_handler(
    bot: Bot,
    msg: Message,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { self_destruction, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_PRIVACY_COUNTER.inc();
    let policy = match lang_code.to_supported_language() {
        RU => RU_POLICY,
        EN => EN_POLICY,
        IT => IT_POLICY,
        FA => FA_POLICY,
        ZH => ZH_POLICY,
    };
    reply_html_ephemeral!(bot, msg, policy, self_destruction, Notice, &lang_code);
    Ok(())
}
