use rand::{Rng, rngs::OsRng};
use teloxide::utils::command::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{Me, User};
use teloxide::types::ParseMode::Html;
use rust_i18n::t;
use crate::{config, help, repo};
use crate::metrics;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    Start,
    Help,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum DickCommands {
    Grow,
    Top,
}

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

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

pub async fn dick_cmd_handler(bot: Bot, msg: Message, cmd: DickCommands,
                              users: repo::Users, dicks: repo::Dicks,
                              config: config::AppConfig) -> HandlerResult {
    let help = match cmd {
        DickCommands::Grow if msg.from().is_some() => {
            metrics::CMD_GROW_COUNTER.inc();

            let from = msg.from().unwrap();
            let increment = OsRng::default().gen_range(config.growth_range);

            users.create_or_update(from.id, from.first_name.clone()).await?;
            let new_length = dicks.create_or_grow(from.id, msg.chat.id, increment).await?;

            let lang_code = ensure_lang_code(Some(from));
            t!("commands.grow.result", locale = lang_code.as_str(), incr = increment, length = new_length)
        },
        DickCommands::Grow => Err("unexpected absence of a FROM field for the /grow command")?,
        DickCommands::Top => {
            metrics::CMD_TOP_COUNTER.inc();

            let lang_code = ensure_lang_code(msg.from());
            let title = t!("commands.top.title", locale = lang_code.as_str());
            let lines = dicks.get_top(msg.chat.id)
                .await?
                .iter().enumerate()
                .map(|(i, d)| {
                    let name = teloxide::utils::html::escape(d.owner_name.as_str());
                    t!("commands.top.line", locale = lang_code.as_str(), n = i+1, name = name, length = d.length)
                })
                .collect::<Vec<String>>();

            if lines.is_empty() {
                t!("commands.top.empty", locale = lang_code.as_str())
            } else {
                format!("{}\n\n{}", title, lines.join("\n"))
            }
        }
    };

    let mut answer = bot.send_message(msg.chat.id, help);
    answer.parse_mode = Some(Html);
    answer.await?;
    Ok(())
}

pub fn ensure_lang_code(user: Option<&User>) -> String {
    user
        .map(|u| {
            u.language_code.clone()
                .or_else(|| {
                    log::warn!("no language_code for {}, using the default", u.id);
                    None
                })
        })
        .flatten()
        .unwrap_or("en".to_owned())
}
