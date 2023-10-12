use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{Me, Message};
use crate::handlers::HandlerResult;
use crate::{help, metrics};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    Start,
    Help,
}

pub async fn help_cmd_handler(bot: Bot, msg: Message, cmd: HelpCommands, me: Me) -> HandlerResult {
    let help = match cmd {
        HelpCommands::Start if msg.from().is_some() => {
            metrics::CMD_START_COUNTER.inc();
            help::get_start_message(msg.from().unwrap(), me)
        },
        HelpCommands::Help => {
            metrics::CMD_HELP_COUNTER.inc();
            help::get_help_message(msg.from(), me)
        }
        HelpCommands::Start => {
            log::warn!("The /start or /help command was invoked without a FROM field for message: {:?}", msg);
            help::get_help_message(msg.from(), me)
        },
    };

    bot.send_message(msg.chat.id, help).await?;
    Ok(())
}
