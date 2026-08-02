use autometrics::autometrics;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::config::MessageGroup::Notice;
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
use crate::help::HelpContainer;
use crate::{metrics, reply_html_ephemeral};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    #[command(description = "help")]
    Help,
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn help_cmd_handler(
    bot: Bot,
    msg: Message,
    container: HelpContainer,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { self_destruction, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_HELP_COUNTER.inc();
    let help = container.get_help_message(&lang_code);
    reply_html_ephemeral!(bot, msg, help, self_destruction, Notice, &lang_code);
    Ok(())
}
