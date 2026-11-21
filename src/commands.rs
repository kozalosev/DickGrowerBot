use rustc_hash::FxHashSet;
use futures::future::join_all;
use once_cell::sync::Lazy;
use rust_i18n::t;
use teloxide::RequestError;
use teloxide::requests::Requester;
use teloxide::types::{BotCommand, BotCommandScope};
use teloxide::utils::command::BotCommands;
use crate::config::{CachedEnvToggles, ThrottledBot};
use crate::handlers::{CleanupCommands, DickCommands, DickOfDayCommands, HelpCommands, ImportCommands, InfoCommands, LanguageCommands, LoanCommands, MentionsCommands, PrivacyCommands, PromoCommands, StartCommands, SupportCommands, TopicsCommands};
use crate::handlers::pvp::{BattleCommands, BattleCommandsNoArgs};
use crate::handlers::stats::StatsCommands;

/// The name of every command the bot answers, without the leading slash and lowercased — the
/// aliases hidden from the menu included, since the bot answers those too.
///
/// Built from the same `bot_commands()` the menu is built from, so a command added there is
/// covered here as well. In a group with several bots most `/…` messages are addressed to someone
/// else, and this is how the bot tells its own apart.
pub static COMMAND_NAMES: Lazy<FxHashSet<String>> = Lazy::new(|| {
    [
        StartCommands::bot_commands(),
        HelpCommands::bot_commands(),
        PrivacyCommands::bot_commands(),
        PromoCommands::bot_commands(),
        SupportCommands::bot_commands(),
        StatsCommands::bot_commands(),
        LanguageCommands::bot_commands(),
        TopicsCommands::bot_commands(),
        CleanupCommands::bot_commands(),
        MentionsCommands::bot_commands(),
        DickCommands::bot_commands(),
        DickOfDayCommands::bot_commands(),
        BattleCommands::bot_commands(),
        BattleCommandsNoArgs::bot_commands(),
        LoanCommands::bot_commands(),
        ImportCommands::bot_commands(),
        InfoCommands::bot_commands(),
    ].concat()
        .into_iter()
        .map(|cmd| cmd.command.trim_start_matches('/').to_lowercase())
        .collect()
});

/// Everything that decides which commands land in the Bot API menu, assembled in `main`. It bundles
/// the per-command env gate ([`CachedEnvToggles`]) with the runtime-derived switches that depend on
/// state only known at startup (e.g. whether an optional integration is available).
pub struct CommandToggles {
    /// The `DISABLE_CMD_*` env gate, applied per command.
    pub env: CachedEnvToggles,
    /// Whether the personal `/language` command is advertised in private chats — it needs the
    /// user-service, so it's hidden when that integration is disabled.
    pub personal_language_enabled: bool,
    /// Whether `/support` is advertised — it needs a chat to relay the messages to, so it's hidden
    /// when `SUPPORT_CHAT_ID` is unset.
    pub support_enabled: bool,
    /// Whether `/cleanup` is advertised — there is nothing for a chat to choose while the
    /// self-destruction is switched off altogether.
    pub cleanup_enabled: bool,
    /// Whether `/mentions` is in the menu.
    pub mentions_enabled: bool,
    /// Whether `/buy` is in the menu — with no auction bot there is nothing to buy.
    pub auction_enabled: bool,
    /// Whether `/donate` is in the menu — with no link there is nowhere to donate.
    pub donate_enabled: bool,
}

/// The menu a command is published in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuScope {
    Private,
    Group,
    /// What the administrators of a group get on top of the group menu.
    Admins,
}

/// A set of commands that decides, command by command, which of them go into a menu.
pub trait PublishedCommands: BotCommands {
    /// Whether this command is in the menu of this scope under these toggles.
    fn is_published(&self, scope: MenuScope, toggles: &CommandToggles) -> bool;

    /// The menu entries of the commands published in this scope.
    fn published_commands(scope: MenuScope, toggles: &CommandToggles) -> Vec<BotCommand> {
        Self::bot_commands()
            .into_iter()
            .filter(|cmd| Self::parse(&cmd.command, "")
                .is_ok_and(|parsed| parsed.is_published(scope, toggles)))
            .collect()
    }
}

pub async fn set_my_commands(
    bot: &ThrottledBot,
    lang_code: &str,
    toggles: &CommandToggles,
) -> Result<(), RequestError> {
    // Telegram only accepts two-letter ISO 639-1 language codes for setMyCommands.
    // Regional variants (e.g. "zh-TW") are rejected, so skip them here — they still
    // apply to all other localized messages, just not the command menu.
    if lang_code.contains('-') {
        tracing::info!(lang_code = %lang_code, "skipping the command registration of a regional locale variant");
        return Ok(());
    }
    let personal_commands = vec![
        HelpCommands::bot_commands(),
        PrivacyCommands::bot_commands(),
        PromoCommands::bot_commands(),
        StatsCommands::bot_commands(),
        MentionsCommands::published_commands(MenuScope::Private, toggles),
        LanguageCommands::published_commands(MenuScope::Private, toggles),
        SupportCommands::published_commands(MenuScope::Private, toggles),
        InfoCommands::published_commands(MenuScope::Private, toggles),
    ];
    let group_commands = vec![
        HelpCommands::bot_commands(),
        DickCommands::bot_commands(),
        DickOfDayCommands::bot_commands(),
        BattleCommands::bot_commands(),
        LoanCommands::bot_commands(),
        StatsCommands::bot_commands(),
        InfoCommands::published_commands(MenuScope::Group, toggles),
    ];
    // The chat-wide /language, /topics, /cleanup and /reset are admin-only, so they live in the
    // admin scope, not the group one.
    let admin_commands = [group_commands.clone(), vec![
        ImportCommands::bot_commands(),
        LanguageCommands::published_commands(MenuScope::Admins, toggles),
        TopicsCommands::bot_commands(),
        CleanupCommands::published_commands(MenuScope::Admins, toggles),
        InfoCommands::published_commands(MenuScope::Admins, toggles),
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
    bot: &ThrottledBot,
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
            cmd.description = t!(&t_key, locale = lang_code).into_owned();
            cmd
        })
        .collect();
    tracing::info!(scope = ?scope, commands = ?commands, "registering the commands");
    let mut request = bot.set_my_commands(commands);
    request.language_code.replace(lang_code.to_owned());
    request.scope.replace(scope);
    request.await?;
    Ok(())
}

#[cfg(test)]
mod test {
    use crate::handlers::{CleanupCommands, InfoCommands, LanguageCommands, MentionsCommands, SupportCommands};
    use super::{CommandToggles, PublishedCommands};
    use super::MenuScope::{Admins, Group, Private};

    const NOTHING: [&str; 0] = [];

    #[test]
    fn switched_commands_follow_their_toggles() {
        let on = toggles(true);
        assert_eq!(names(MentionsCommands::published_commands(Private, &on)), ["/mentions"]);
        assert_eq!(names(SupportCommands::published_commands(Private, &on)), ["/support"]);
        assert_eq!(names(CleanupCommands::published_commands(Admins, &on)), ["/cleanup"]);
        assert_eq!(names(LanguageCommands::published_commands(Private, &on)), ["/language"]);
        assert_eq!(names(InfoCommands::published_commands(Private, &on)), ["/buy", "/donate"]);

        let off = toggles(false);
        assert_eq!(names(MentionsCommands::published_commands(Private, &off)), NOTHING);
        assert_eq!(names(SupportCommands::published_commands(Private, &off)), NOTHING);
        assert_eq!(names(CleanupCommands::published_commands(Admins, &off)), NOTHING);
        assert_eq!(names(LanguageCommands::published_commands(Private, &off)), NOTHING);
        assert_eq!(names(LanguageCommands::published_commands(Admins, &off)), ["/language"]);
        assert_eq!(names(InfoCommands::published_commands(Private, &off)), NOTHING);
    }

    #[test]
    fn info_commands_are_published_where_they_make_sense() {
        let on = toggles(true);
        let private = names(InfoCommands::published_commands(Private, &on));
        let group = names(InfoCommands::published_commands(Group, &on));
        let admins = names(InfoCommands::published_commands(Admins, &on));
        assert_eq!(private, ["/buy", "/donate"]);
        assert_eq!(group, ["/voucher", "/buy", "/donate"]);
        assert_eq!(admins, ["/reset"]);
    }

    fn toggles(enabled: bool) -> CommandToggles {
        CommandToggles {
            env: Default::default(),
            personal_language_enabled: enabled,
            support_enabled: enabled,
            cleanup_enabled: enabled,
            mentions_enabled: enabled,
            auction_enabled: enabled,
            donate_enabled: enabled,
        }
    }

    /// The commands as the menu shows them: `set_commands` drops the undescribed aliases.
    fn names(commands: Vec<teloxide::types::BotCommand>) -> Vec<String> {
        commands.into_iter()
            .filter(|cmd| !cmd.description.is_empty())
            .map(|cmd| cmd.command)
            .collect()
    }
}
