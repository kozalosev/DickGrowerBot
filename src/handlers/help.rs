use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::help::HelpContainer;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    #[command(description = "help")]
    Help,
}

pub async fn help_cmd_handler(bot: Bot, msg: Message, container: HelpContainer) -> HandlerResult {
    let lang_code = ensure_lang_code(msg.from());
    let help = container.get_help_message(lang_code).to_owned();
    reply_html(bot, msg, help).await?;
    Ok(())
}
