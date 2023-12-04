use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::metrics;
use crate::help::HelpContainer;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    Start,
    #[command(description = "help")]
    Help,
}

pub async fn help_cmd_handler(bot: Bot, msg: Message, cmd: HelpCommands, container: HelpContainer) -> HandlerResult {
    let lang_code = ensure_lang_code(msg.from());
    let help = match cmd {
        HelpCommands::Start if msg.from().is_some() => {
            metrics::CMD_START_COUNTER.inc();
            let username = teloxide::utils::html::escape(&msg.from().unwrap().first_name);
            container.get_start_message(username, lang_code)
        },
        HelpCommands::Help => {
            metrics::CMD_HELP_COUNTER.inc();
            container.get_help_message(lang_code).to_owned()
        }
        HelpCommands::Start => {
            log::warn!("The /start or /help command was invoked without a FROM field for message: {:?}", msg);
            container.get_help_message(lang_code).to_owned()
        },
    };
    reply_html(bot, msg, help).await?;
    Ok(())
}
