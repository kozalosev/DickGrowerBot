use std::collections::{HashMap, HashSet};
use anyhow::anyhow;
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{ChatId, Message, UserId};
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::repo;

const ORIGINAL_BOT_USERNAME: &str = "@pipisabot";   // TODO: support @kraft28_bot as well

static TOP_LINE_REGEXP: Lazy<Regex> = Lazy::new(|| {
    // TODO: load from env var
    Regex::new(r"\d{1,2}\|(?<name>.{1,13})(\.{3})? — (?<length>\d+) см.")
        .expect("TOP_LINE_REGEXP is invalid")
});

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum ImportCommands {
    Import
}

type Username = String;

struct OriginalUser {
    name: String,
    length: u32
}

struct ChatMember {
    uid: UserId,
    full_name: String,
}

struct UserInfo {
    uid: UserId,
    name: String,
    length: u32
}

struct ImportResult {
    imported: Vec<UserInfo>,
    already_present: Vec<UserInfo>,
    not_found: Vec<Username>,
}

#[derive(Debug, derive_more::Display)]
struct InvalidLines;

#[derive(strum_macros::Display)]
#[strum(serialize_all="snake_case")]
enum BeforeImportCheckErrors {
    NotGroupChat,
    NotAdmin,
    NotReply,
    Other(anyhow::Error)
}

impl From<anyhow::Error> for BeforeImportCheckErrors {
    fn from(value: anyhow::Error) -> Self {
        Self::Other(value)
    }
}

struct Repositories<'a> {
    users: &'a repo::Users,
    imports: &'a repo::Imports,
}

pub async fn import_cmd_handler(bot: Bot, msg: Message,
                                users: repo::Users, imports: repo::Imports) -> HandlerResult {
    let lang_code = ensure_lang_code(msg.from());
    let answer = match check_and_parse_message(&bot, &msg).await {
        Ok(txt) => {
            let repos = Repositories {
                users: &users,
                imports: &imports,
            };
            match import_impl(repos, msg.chat.id, txt).await {
                Ok(r) => {
                    let imported = r.imported.into_iter()
                        .map(|u| t!("commands.import.result.line.imported",
                            name = u.name, length = u.length,
                            locale = lang_code.as_str()))
                        .collect::<Vec<String>>()
                        .join("\n");
                    let already_present = r.already_present.into_iter()
                        .map(|u| t!("commands.import.result.line.already_present",
                            name = u.name, length = u.length,
                            locale = lang_code.as_str()))
                        .collect::<Vec<String>>()
                        .join("\n");
                    let not_found = r.not_found.into_iter()
                        .map(|name| t!("commands.import.result.line.not_found",
                            name = name,
                            locale = lang_code.as_str()))
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
                        .collect::<Vec<String>>()
                        .join("\n\n")
                }
                Err(e) => if let Some(InvalidLines) = e.downcast_ref() {
                    log::error!("Invalid lines: {msg:?}");
                    t!("commands.import.errors.invalid_lines", locale = lang_code.as_str())
                } else {
                    Err(e)?
                }
            }

        },
        Err(BeforeImportCheckErrors::Other(e)) => Err(e)?,
        Err(BeforeImportCheckErrors::NotReply) => t!(format!("commands.import.errors.not_reply").as_str(),
            origin_bot = ORIGINAL_BOT_USERNAME,
            locale = lang_code.as_str()),
        Err(e) => t!(format!("commands.import.errors.{e}").as_str(),
            locale = lang_code.as_str()),
    };
    reply_html(bot, msg, answer).await
}

async fn check_and_parse_message<'a>(bot: &Bot, msg: &Message) -> Result<String, BeforeImportCheckErrors> {
    if msg.chat.is_private() || msg.chat.is_channel() {
        return Err(BeforeImportCheckErrors::NotGroupChat)
    }

    let admin_ids = bot.get_chat_administrators(msg.chat.id)
        .await
        .map_err(|e| BeforeImportCheckErrors::Other(anyhow!(e)))?
        .into_iter()
        .map(|m| m.user.id);
    let from_id = msg.from()
        .ok_or(BeforeImportCheckErrors::Other(anyhow!("not from a user")))?
        .id;
    let invoked_by_admin = admin_ids.into_iter().any(|id| id == from_id);
    if !invoked_by_admin {
        return Err(BeforeImportCheckErrors::NotAdmin)
    }

    let text = msg.reply_to_message()
        .filter(check_reply_source)
        .and_then(|reply| reply.text());
    let text = match text {
        None => return Err(BeforeImportCheckErrors::NotReply),
        Some(text) => text.to_owned()
    };

    Ok(text)
}

fn check_reply_source(reply: &&Message) -> bool {
    reply.via_bot.clone()
        .and_then(|u| u.username)
        .filter(|n| n == ORIGINAL_BOT_USERNAME)
        .is_some()
}

async fn import_impl<'a>(repos: Repositories<'a>, chat_id: ChatId, text: String) -> anyhow::Result<ImportResult> {
    let members: HashMap<String, ChatMember> = repos.users.get_chat_members(chat_id)
        .await?.into_iter()
        .map(|m| {
            let uid = m.uid.try_into().expect("couldn't convert uid to u64");
            let short_name = if m.name.len() > 13 {
                m.name[0..13].to_owned()
            } else {
                m.name.to_owned()
            };
            let member = ChatMember {
                uid: UserId(uid),
                full_name: m.name
            };
            (short_name, member)
        })
        .collect();
    let member_names: HashSet<_> = HashSet::from_iter(members.keys());

    let top: Vec<Option<Captures>> = text.lines()
        .skip(2)    // TODO: parametrize by env var
        .map(|pos| TOP_LINE_REGEXP.captures(pos))
        .collect();
    let invalid_lines_present = top.iter()
        .any(|pos| pos.is_none());
    if invalid_lines_present {
        return Err(anyhow!(InvalidLines))
    }

    let top: Vec<OriginalUser> = top.into_iter()
        .filter_map(map_user)
        .collect();
    let (existing, not_existing): (Vec<OriginalUser>, Vec<OriginalUser>) = top.into_iter()
        .partition(|u| member_names.contains(&u.name));
    let existing: Vec<UserInfo> = existing.into_iter()
        .map(|u| {
            let member = &members[&u.name];
            UserInfo {
                uid: member.uid,
                name: member.full_name.clone(),
                length: u.length
            }
        })
        .collect();
    let not_found = not_existing.into_iter()
        .map(|u| teloxide::utils::html::escape(&u.name))
        .collect();

    let imported_uids: HashSet<UserId> = repos.imports.get_imported_users(chat_id)
        .await?.into_iter()
        .filter_map(|u| u.uid.try_into().ok())
        .map(|uid| UserId(uid))
        .collect();

    let (already_present, to_import): (Vec<UserInfo>, Vec<UserInfo>) = existing.into_iter()
        .partition(|u| imported_uids.contains(&u.uid));

    let users: Vec<repo::ExternalUser> = to_import.iter()
        .filter_map(|u| repo::ExternalUser::new(u.uid, u.length).ok())
        .collect();
    if users.len() != to_import.len() {
        return Err(anyhow!("couldn't convert integers for external users"))
    }
    repos.imports.import(chat_id, &users).await?;

    Ok(ImportResult {
        imported: to_import,
        already_present,
        not_found
    })
}

fn map_user(capture: Option<Captures>) -> Option<OriginalUser> {
    let pos = capture?;
    if let (Some(name), Some(length)) = (pos.name("name"), pos.name("length")) {
        let name = name.as_str().to_owned();
        let length = length.as_str().parse().ok()?;
        return Some(OriginalUser { name, length })
    }
    None
}
