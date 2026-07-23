use futures::future::join_all;
use rust_i18n::t;
use teloxide::{Bot, RequestError};
use teloxide::requests::Requester;
use teloxide::types::{BotCommand, BotCommandScope};
use teloxide::utils::command::BotCommands;
use crate::config::CachedEnvToggles;
use crate::handlers::{DickCommands, DickOfDayCommands, HelpCommands, ImportCommands, LanguageCommands, LoanCommands, PrivacyCommands, PromoCommands};
use crate::handlers::pvp::BattleCommands;
use crate::handlers::stats::StatsCommands;

/// Everything that decides which commands land in the Bot API menu, assembled in `main`. It bundles
/// the per-command env gate ([`CachedEnvToggles`]) with the runtime-derived switches that depend on
/// state only known at startup (e.g. whether an optional integration is available).
pub struct CommandToggles {
    /// The `DISABLE_CMD_*` env gate, applied per command.
    pub env: CachedEnvToggles,
    /// Whether the personal `/language` command is advertised in private chats — it needs the
    /// user-service, so it's hidden when that integration is disabled.
    pub personal_language_enabled: bool,
}

pub async fn set_my_commands(
    bot: &Bot,
    lang_code: &str,
    toggles: &CommandToggles,
) -> Result<(), RequestError> {
    // Telegram only accepts two-letter ISO 639-1 language codes for setMyCommands.
    // Regional variants (e.g. "zh-TW") are rejected, so skip them here — they still
    // apply to all other localized messages, just not the command menu.
    if lang_code.contains('-') {
        log::info!("Skipping command registration for regional locale variant {lang_code}");
        return Ok(());
    }
    let personal_commands = vec![
        HelpCommands::bot_commands(),
        PrivacyCommands::bot_commands(),
        PromoCommands::bot_commands(),
        StatsCommands::bot_commands(),
        if toggles.personal_language_enabled { LanguageCommands::bot_commands() } else { Vec::new() },
    ];
    let group_commands = vec![
        HelpCommands::bot_commands(),
        DickCommands::bot_commands(),
        DickOfDayCommands::bot_commands(),
        BattleCommands::bot_commands(),
        LoanCommands::bot_commands(),
        StatsCommands::bot_commands(),
    ];
    // The chat-wide /language is admin-only, so it lives in the admin scope, not the group scope.
    let admin_commands = [group_commands.clone(), vec![
        ImportCommands::bot_commands(),
        LanguageCommands::bot_commands(),
    ]].concat();

    let requests = vec![
        set_commands(bot, personal_commands, BotCommandScope::AllPrivateChats, lang_code, &toggles.env),
        set_commands(bot, group_commands, BotCommandScope::AllGroupChats, lang_code, &toggles.env),
        set_commands(bot, admin_commands, BotCommandScope::AllChatAdministrators, lang_code, &toggles.env),
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

async fn set_commands(
    bot: &Bot,
    commands: Vec<Vec<BotCommand>>,
    scope: BotCommandScope,
    lang_code: &str,
    toggles: &CachedEnvToggles,
) -> Result<(), RequestError> {
    let commands: Vec<BotCommand> = commands
        .concat()
        .into_iter()
        .filter(|cmd| !cmd.description.is_empty())
        .filter(|cmd| toggles.enabled(&cmd.description))
        .map(|mut cmd| {
            let t_key = format!("commands.{}.description", cmd.description);
            cmd.description = t!(&t_key, locale = lang_code).to_string();
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
