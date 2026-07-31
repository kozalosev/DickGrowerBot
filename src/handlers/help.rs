use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::config::MessageGroup::Notice;
use crate::handlers::{HandlerResult, reply_html};
use crate::handlers::utils::SelfDestructionService;
use crate::help::HelpContainer;
use crate::users::LanguageResolver;
use crate::{metrics, reply_html_ephemeral};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    #[command(description = "help")]
    Help,
}

pub async fn help_cmd_handler(
    bot: Bot,
    msg: Message,
    container: HelpContainer,
    lang_resolver: LanguageResolver,
    self_destruction: SelfDestructionService,
) -> HandlerResult {
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_HELP_COUNTER.inc();
    let help = container.get_help_message(&lang_code);
    reply_html_ephemeral!(bot, msg, help, self_destruction, Notice, &lang_code);
    Ok(())
}
