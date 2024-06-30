use futures::future::join_all;
use rust_i18n::t;
use teloxide::{Bot, RequestError};
use teloxide::requests::Requester;
use teloxide::types::{BotCommand, BotCommandScope};
use teloxide::utils::command::BotCommands;
use crate::config::CachedEnvToggles;
use crate::handlers::{DickCommands, DickOfDayCommands, HelpCommands, ImportCommands, LoanCommands, PromoCommands};
use crate::handlers::pvp::BattleCommands;
use crate::handlers::stats::StatsCommands;

pub async fn set_my_commands(bot: &Bot, lang_code: &str, toggles: &CachedEnvToggles) -> Result<(), RequestError> {
    let personal_commands = vec![
        HelpCommands::bot_commands(),
        PromoCommands::bot_commands(),
    ];
    let group_commands = vec![
        HelpCommands::bot_commands(),
        DickCommands::bot_commands(),
        DickOfDayCommands::bot_commands(),
        BattleCommands::bot_commands(),
        LoanCommands::bot_commands(),
        StatsCommands::bot_commands(),
    ];
    let admin_commands = [group_commands.clone(), vec![
        ImportCommands::bot_commands(),
    ]].concat();

    let requests = vec![
        set_commands(bot, personal_commands, BotCommandScope::AllPrivateChats, lang_code, toggles),
        set_commands(bot, group_commands, BotCommandScope::AllGroupChats, lang_code, toggles),
        set_commands(bot, admin_commands, BotCommandScope::AllChatAdministrators, lang_code, toggles),
    ];
    join_all(requests)
        .await
        .into_iter()
        .filter(|resp| resp.is_err())
        .map(|resp| Err(resp.unwrap_err()))
        .take(1)
        .last()
        .unwrap_or(Ok(()))
}

async fn set_commands(bot: &Bot, commands: Vec<Vec<BotCommand>>, scope: BotCommandScope, lang_code: &str, toggles: &CachedEnvToggles) -> Result<(), RequestError> {
    let commands: Vec<BotCommand> = commands
        .concat()
        .into_iter()
        .filter(|cmd| !cmd.description.is_empty())
        .filter(|cmd| toggles.enabled(&cmd.description))
        .map(|mut cmd| {
            cmd.description = t!(&format!("commands.{}.description", cmd.description), locale = lang_code);
            cmd
        })
        .collect();
    log::info!("Registering commands for scope {scope:?}: {commands:?}");
    let mut request = bot.set_my_commands(commands);
    request.language_code.replace(lang_code.to_owned());
    request.scope.replace(scope);
    request.await?;
    Ok(())
}
