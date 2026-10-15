use autometrics::autometrics;
use std::collections::HashSet;
use anyhow::{anyhow, bail};
use itertools::Itertools;
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{ChatId, Me, Message, UserId};
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
use crate::{metrics, reply_html, repo};
use crate::domain::objects::{ExternalUser, OriginalBotKind};
use crate::domain::primitives::{Length, UserId as DomainUserId, Username};

pub const ORIGINAL_BOT_USERNAMES: [&str; 2] = ["pipisabot", "kraft28_bot"];

const MAX_MEMBERS_FOR_IMPORT: usize = 10;

static TOP_LINE_REGEXP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{1,3}((\. )|\|)(?<name>.+?) — (?<length>\d+) см\.?")
        .expect("TOP_LINE_REGEXP is invalid")
});

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum ImportCommands {
    #[command(description = "import")]
    Import
}

struct ParseResult(OriginalBotKind, String);

#[derive(strum_macros::Display)]
#[strum(serialize_all="snake_case")]
enum BeforeImportCheckErrors {
    NotAdmin,
    BotNotAdmin,
    NotReply,
    LegacyGroup,
    TooManyMembers,
    Other(anyhow::Error)
}

impl <T: Into<anyhow::Error>> From<T> for BeforeImportCheckErrors {
    fn from(value: T) -> Self {
        Self::Other(anyhow!(value))
    }
}

struct OriginalUser {
    name: Username,
    length: u32
}

struct ChatMember {
    uid: UserId,
    full_name: String,
}

struct UserInfo {
    uid: UserId,
    name: Username,
    length: u32
}

struct ImportResult {
    imported: Vec<UserInfo>,
    already_present: Vec<UserInfo>,
    not_found: Vec<Username>,
}

#[derive(Debug, derive_more::Display, derive_more::Error)]
enum ImportError {
    #[display("no leaderboard entries found")]
    NothingFound,
    #[display("ambiguous names: {_0:?}")]
    AmbiguousNames(#[error(not(source))] Vec<Username>),
    #[display("invalid lines: {_0:?}")]
    InvalidLines(#[error(not(source))] Vec<String>),
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn import_cmd_handler(
    bot: Bot,
    bot_info: Me,
    msg: Message,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let parsed = check_and_parse_message(&bot, bot_info.id, &msg, &repos).await;
    let source = parsed.as_ref().ok().map(|p| p.0);
    metrics::CMD_IMPORT.invoked(source);
    let answer = match parsed {
        Ok(parsed) => {
            match import_impl(&repos, msg.chat.id, parsed).await {
                Ok(r) => {
                    metrics::CMD_IMPORT.finished(source);
                    let imported = r.imported.into_iter()
                        .map(|u| t!("commands.import.result.line.imported", locale = &lang_code,
                            name = u.name.escaped(),
                            length = u.length))
                        .map(|cow_str| cow_str.to_string())
                        .collect::<Vec<String>>()
                        .join("\n");
                    let already_present = r.already_present.into_iter()
                        .map(|u| t!("commands.import.result.line.already_present", locale = &lang_code,
                            name = u.name.escaped(),
                            length = u.length))
                        .map(|cow_str| cow_str.to_string())
                        .collect::<Vec<String>>()
                        .join("\n");
                    let not_found = r.not_found.into_iter()
                        .map(|name| t!("commands.import.result.line.not_found", locale = &lang_code,
                            name = name.escaped()))
                        .map(|cow_str| cow_str.to_string())
                        .collect::<Vec<String>>()
                        .join("\n");

                    [
                        ("imported", imported),
                        ("already_present", already_present),
                        ("not_found", not_found),
                    ].into_iter()
                        .filter(|t| !t.1.is_empty())
                        .map(|t| {
                            let title_key = format!("commands.import.result.titles.{}", t.0);
                            let title = t!(&title_key, locale = &lang_code);
                            format!("{}\n{}", title, t.1)
                        })
                        .map(|cow_str| cow_str.to_string())
                        .collect::<Vec<String>>()
                        .join("\n\n")
                }
                Err(e) => match e.downcast::<ImportError>() {
                    Ok(ImportError::NothingFound) => {
                        metrics::CMD_IMPORT.rejected(source);
                        t!("commands.import.errors.nothing_found", locale = &lang_code).to_string()
                    },
                    Ok(ImportError::AmbiguousNames(names)) => {
                        metrics::CMD_IMPORT.rejected(source);
                        let names = names.into_iter()
                            .map(|name| name.escaped())
                            .collect::<Vec<_>>()
                            .join(", ");
                        t!("commands.import.errors.ambiguous_names", locale = &lang_code, names = names).to_string()
                    },
                    Ok(ImportError::InvalidLines(lines)) => {
                        metrics::CMD_IMPORT.rejected(source);
                        tracing::error!(lines = ?lines, "invalid lines in the imported message");
                        let invalid_lines = lines.iter()
                            .map(|line| t!("commands.import.errors.invalid_lines.line", locale = &lang_code,
                                line = line))
                            .map(|cow_str| cow_str.to_string())
                            .collect::<Vec<String>>()
                            .join("\n");
                        t!("commands.import.errors.invalid_lines.template", locale = &lang_code,
                            invalid_lines = invalid_lines).to_string()
                    },
                    Err(e) => {
                        metrics::CMD_IMPORT.failed(source);
                        Err(e)?
                    },
                }
            }

        },
        Err(BeforeImportCheckErrors::Other(e)) => {
            metrics::CMD_IMPORT.failed(source);
            Err(e)?
        },
        Err(BeforeImportCheckErrors::NotReply) => {
            metrics::CMD_IMPORT.rejected(source);
            let origin_bots = ORIGINAL_BOT_USERNAMES.map(|name| format!("@{name}")).join(", ");
            t!("commands.import.errors.not_reply", locale = &lang_code,
                origin_bots = origin_bots).to_string()
        },
        Err(BeforeImportCheckErrors::LegacyGroup) => {
            metrics::CMD_IMPORT.rejected(source);
            t!("commands.import.errors.legacy_group", locale = &lang_code).to_string()
        },
        Err(e) => {
            metrics::CMD_IMPORT.rejected(source);
            let t_key = format!("commands.import.errors.{e}");
            t!(&t_key, locale = &lang_code).to_string()
        },
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(msg)))]
async fn check_and_parse_message(
    bot: &Bot,
    bot_id: UserId,
    msg: &Message,
    repos: &repo::Repositories,
) -> Result<ParseResult, BeforeImportCheckErrors> {
    if msg.chat.is_group() {
        return Err(BeforeImportCheckErrors::LegacyGroup);
    }

    let admin_ids: Vec<UserId> = bot.get_chat_administrators(msg.chat.id)
        .await?
        .into_iter()
        .map(|m| m.user.id)
        .collect();
    let from_id = msg.from.as_ref()
        .ok_or(BeforeImportCheckErrors::Other(anyhow!("not from a user")))?
        .id;
    if !admin_ids.contains(&from_id) {
        return Err(BeforeImportCheckErrors::NotAdmin)
    }
    // Reading another bot's message only works if this bot is itself an admin
    if !admin_ids.contains(&bot_id) {
        return Err(BeforeImportCheckErrors::BotNotAdmin)
    }

    let chat_id_kind = msg.chat.id.into();
    let members_count = repos.users.get_chat_members(&chat_id_kind).await?.len();
    if members_count > MAX_MEMBERS_FOR_IMPORT {
        return Err(BeforeImportCheckErrors::TooManyMembers)
    }

    msg.reply_to_message()
        .filter(|m| m.forward_origin().is_none())
        .and_then(check_reply_source_and_text)
        .ok_or(BeforeImportCheckErrors::NotReply)
}

fn check_reply_source_and_text(reply: &Message) -> Option<ParseResult> {
    reply.from.as_ref()
        .filter(|u| u.is_bot)
        .and_then(|u| u.username.as_ref())
        .filter(|name| ORIGINAL_BOT_USERNAMES.contains(&name.as_ref()))
        .and_then(|name| {
            let name = name.as_str().try_into()
                .map_err(|name| tracing::error!(name = %name, "couldn't convert the name"))
                .ok();
            if let (Some(name), Some(text)) = (name, reply.text()) {
                Some(ParseResult(name, text.to_owned()))
            } else {
                None
            }
        })
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = chat_id.0))]
async fn import_impl(repos: &repo::Repositories, chat_id: ChatId, parsed: ParseResult) -> anyhow::Result<ImportResult> {
    let top = parse_import_users(&parsed.1)?;
    let chat_id_kind = chat_id.into();
    let members: Vec<ChatMember> = repos.users.get_chat_members(&chat_id_kind)
        .await?.into_iter()
        .map(|m| ChatMember {
            uid: m.uid.into(),
            full_name: m.name.value().to_owned()
        })
        .collect();
    let (existing, not_found) = resolve_users(&parsed.0, top, &members)?;

    let imported_uids: HashSet<UserId> = repos.import.get_imported_users(chat_id)
        .await?.into_iter()
        .map(|u| u.uid.into())
        .collect();

    let (already_present, to_import): (Vec<UserInfo>, Vec<UserInfo>) = existing.into_iter()
        .partition(|u| imported_uids.contains(&u.uid));

    let users: Vec<ExternalUser> = to_import.iter()
        .map(|u| ExternalUser::new(DomainUserId::from(u.uid), Length::new(u.length.into())))
        .collect();
    repos.import.import(chat_id, &users).await?;

    Ok(ImportResult {
        imported: to_import,
        already_present,
        not_found
    })
}

/// Splits the leaderboard into the users found in the chat and the names that were not. A name that
/// is shown more than once, or fits more than one member, cannot be attributed to anybody, so it
/// fails the whole import.
fn resolve_users(
    kind: &OriginalBotKind,
    top: Vec<OriginalUser>,
    members: &[ChatMember],
) -> Result<(Vec<UserInfo>, Vec<Username>), ImportError> {
    let mut shown: HashSet<Username> = HashSet::new();
    let mut ambiguous: Vec<Username> = Vec::new();
    let mut found: Vec<UserInfo> = Vec::new();
    let mut not_found: Vec<Username> = Vec::new();

    for user in top {
        let repeated = !shown.insert(user.name.clone());
        let mut fitting = members.iter()
            .filter(|m| kind.matches_name(&m.full_name, user.name.value()));
        let member = fitting.next();
        if repeated || fitting.next().is_some() {
            ambiguous.push(user.name.clone());
            continue
        }
        match member {
            Some(member) => found.push(UserInfo {
                uid: member.uid,
                name: Username::new(member.full_name.clone()),
                length: user.length,
            }),
            None => not_found.push(user.name),
        }
    }

    if ambiguous.is_empty() {
        Ok((found, not_found))
    } else {
        Err(ImportError::AmbiguousNames(ambiguous.into_iter().unique().collect()))
    }
}

fn parse_import_users(text: &str) -> anyhow::Result<Vec<OriginalUser>> {
    let top = parse_top_lines(text);
    let invalid_lines: Vec<String> = top.iter()
        .filter(|pos| pos.is_err())
        .map(|pos| pos.as_ref().unwrap_err().clone())
        .collect();
    if !invalid_lines.is_empty() {
        bail!(ImportError::InvalidLines(invalid_lines))
    }

    let top: Vec<OriginalUser> = top.into_iter()
        .flatten()
        .filter_map(map_user)
        .collect();
    if top.is_empty() {
        bail!(ImportError::NothingFound)
    }
    Ok(top)
}

fn parse_top_lines(text: &str) -> Vec<Result<Captures<'_>, String>> {
    text.lines()
        .skip_while(|s| !TOP_LINE_REGEXP.is_match(s))
        .take_while(|s| !s.trim().is_empty())
        .map(|pos| TOP_LINE_REGEXP.captures(pos).ok_or(pos.to_owned()))
        .collect()
}

fn map_user(pos: Captures) -> Option<OriginalUser> {
    if let (Some(name), Some(length)) = (pos.name("name"), pos.name("length")) {
        let name = Username::new(name.as_str().to_owned());
        let length = length.as_str().parse().ok()?;
        return Some(OriginalUser { name, length })
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_top_lines_ignores_blank_lines_and_footer() {
        let text = "Топ 10 игроков 🔝\n\n1|Leonid SadBot🍆... — 3 см.\n\n\nраздача см в канале)";

        let top = parse_top_lines(text);

        assert_eq!(top.len(), 1);
        let user = map_user(top.into_iter().next().unwrap().expect("line should be valid"))
            .expect("captures should map to a user");
        assert_eq!(user.name.value(), "Leonid SadBot🍆...");
        assert_eq!(user.length, 3);
    }

    #[test]
    fn parse_top_lines_reports_invalid_entry_within_the_list() {
        let text = "Топ 10 игроков 🔝\n\n1|Leonid SadBot🍆... — 3 см.\nnot a valid line\n2|Another — 5 см.";

        let top = parse_top_lines(text);

        assert_eq!(top.len(), 3);
        assert!(top[0].is_ok());
        assert_eq!(top[1].as_ref().unwrap_err(), "not a valid line");
        assert!(top[2].is_ok());
    }

    #[test]
    fn the_username_list_names_every_original_bot() {
        use strum::IntoEnumIterator;

        let names: Vec<String> = OriginalBotKind::iter()
            .map(|kind| kind.to_string())
            .collect();
        assert_eq!(names, ORIGINAL_BOT_USERNAMES);
    }

    fn member(uid: u64, name: &str) -> ChatMember {
        ChatMember { uid: UserId(uid), full_name: name.to_owned() }
    }

    fn user(name: &str) -> OriginalUser {
        OriginalUser { name: Username::new(name.to_owned()), length: 1 }
    }

    #[test]
    fn two_members_fitting_one_first_name_are_ambiguous() {
        let members = [member(1, "Leonid Smith"), member(2, "Leonid Jones"), member(3, "Alice")];
        let top = vec![user("Leonid"), user("Alice")];
        let result = resolve_users(&OriginalBotKind::Kraft28, top, &members);
        assert!(matches!(result, Err(ImportError::AmbiguousNames(names)) if names == [Username::from("Leonid")]));
    }

    #[test]
    fn a_name_shown_twice_is_ambiguous_even_with_no_member() {
        let members = [member(1, "Alice")];
        let top = vec![user("Bob"), user("Bob"), user("Alice")];
        let result = resolve_users(&OriginalBotKind::Kraft28, top, &members);
        assert!(matches!(result, Err(ImportError::AmbiguousNames(names)) if names == [Username::from("Bob")]));
    }

    #[test]
    fn pipisa_finds_cut_names_with_random_emoji() {
        let members = [member(1, "Leonid SadBot Rust'a'man"), member(2, "Leonid Kozarin")];
        let top = vec![user("Leonid SadBot🍆..."), user("Leonid Kozari🤥...")];
        let (found, not_found) = resolve_users(&OriginalBotKind::Pipisa, top, &members)
            .expect("the names are distinct");
        let found_uids: Vec<UserId> = found.iter().map(|u| u.uid).collect();
        assert_eq!(found_uids, [UserId(1), UserId(2)]);
        assert!(not_found.is_empty());
    }

    #[test]
    fn two_members_with_the_same_beginning_are_ambiguous_for_a_cut_name() {
        let members = [member(1, "Leonid SadBot One"), member(2, "Leonid SadBot Two")];
        let top = vec![user("Leonid SadBot🍆...")];
        let result = resolve_users(&OriginalBotKind::Pipisa, top, &members);
        assert!(matches!(result, Err(ImportError::AmbiguousNames(_))));
    }

    #[test]
    fn distinct_names_are_split_into_found_and_not_found() {
        let members = [member(1, "Leonid Smith"), member(2, "Alice")];
        let top = vec![user("Leonid"), user("Alice"), user("Bob")];
        let (found, not_found) = resolve_users(&OriginalBotKind::Kraft28, top, &members)
            .expect("the names are distinct");
        let found_uids: Vec<UserId> = found.iter().map(|u| u.uid).collect();
        assert_eq!(found_uids, [UserId(1), UserId(2)]);
        assert_eq!(found[0].name.value(), "Leonid Smith");
        let not_found: Vec<&str> = not_found.iter().map(|n| n.value()).collect();
        assert_eq!(not_found, ["Bob"]);
    }

    #[test]
    fn parses_kraft28_lines_ending_at_centimetres() {
        let text = "Рейтинг гравців:\n\n1. Alice — 97 см\n2. Bob — 79 см\n3. Carol — 3 см";
        let users = parse_import_users(text).expect("the leaderboard should be recognised");
        assert_eq!(users.len(), 3);
        assert_eq!(users[0].name.value(), "Alice");
        assert_eq!(users[0].length, 97);
        assert_eq!(users[2].length, 3);
    }

    #[test]
    fn a_reply_without_leaderboard_entries_is_an_error() {
        let err = parse_import_users("Рейтинг гравців:\n\nNo entries yet")
            .err().expect("an empty import must be rejected");
        assert!(matches!(err.downcast_ref(), Some(ImportError::NothingFound)));
    }
}
