use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::domain::LanguageCode;
use crate::handlers::{HandlerResult, reply_html};
use crate::help::HelpContainer;
use crate::reply_html;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    #[command(description = "help")]
    Help,
}

#[tracing::instrument]
pub async fn help_cmd_handler(bot: Bot, msg: Message, container: HelpContainer) -> HandlerResult {
    let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
    let help = container.get_help_message(lang_code);
    reply_html!(bot, msg, help);
    Ok(())
}
