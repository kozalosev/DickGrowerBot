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
    Regex::new(r"\d{1,2}\|(?<name>.+) — (?<length>\d+) см.")
        .expect("TOP_LINE_REGEXP is invalid")
});

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum ImportCommands {
    Import
}

type Username = String;

pub struct OriginalUser {
    name: String,
    length: u32
}

struct ImportResult {
    found: Vec<OriginalUser>,
    not_found: Vec<Username>,
}

#[derive(Debug, derive_more::Display)]
struct InvalidLines;

#[derive(strum_macros::Display)]
#[strum(serialize_all="snake_case")]
enum BeforeImportCheckErrors {
    NotAdmin,
    NotReply,
    AlreadyImported,
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
    let repos = Repositories {
        users: &users,
        imports: &imports,
    };

    let answer = match check_params_and_parse_message(&bot, &msg, &repos).await {
        Ok(txt) => {
            let result = import_impl(repos, msg.chat.id, txt).await?;
            let imported = result.found.into_iter()
                .map(|u| t!("commands.import.result.imported",
                    name = u.name, length = u.length,
                    locale = lang_code.as_str()))
                .collect::<Vec<String>>()
                .join("\n");
            let not_found = result.not_found.into_iter()
                .map(|name| t!("commands.import.result.not_found",
                    name = name,
                    locale = lang_code.as_str()))
                .collect::<Vec<String>>()
                .join("\n");
            t!("commands.import.result.template", imported = imported, not_found = not_found,
                locale = lang_code.as_str())
        },
        Err(BeforeImportCheckErrors::Other(e)) => Err(e)?,
        Err(e) => t!(format!("commands.import.errors.{e}").as_str(),
            locale = lang_code.as_str()),
    };
    reply_html(bot, msg, answer).await
}

async fn check_params_and_parse_message<'a>(bot: &Bot, msg: &Message, repos: &Repositories<'a>) -> Result<String, BeforeImportCheckErrors> {
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

    let already_imported = repos.imports.were_dicks_already_imported(msg.chat.id).await?;
    if already_imported {
        return Err(BeforeImportCheckErrors::AlreadyImported)
    }

    Ok(text)
}

fn check_reply_source(reply: &&Message) -> bool {
    reply.via_bot.clone()
        .and_then(|u| u.username)
        .filter(|n| n == ORIGINAL_BOT_USERNAME)
        .is_some()
}

async fn import_impl<'a>(repos: Repositories<'a>, chat_id: ChatId, text: String) -> anyhow::Result<ImportResult> {
    let members: HashMap<String, UserId> = repos.users.get_chat_members(chat_id)
        .await?.into_iter()
        .map(|m| {
            let uid = m.uid.try_into().expect("couldn't convert uid to u64");
            (m.name, UserId(uid))
        })
        .collect();
    let member_names: HashSet<_> = HashSet::from_iter(members.keys());  // TODO: cloned()?

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
        .filter_map(map_users)
        .collect();
    let (existing, not_existing): (Vec<OriginalUser>, Vec<OriginalUser>) = top.into_iter()
        .partition(|u| member_names.contains(&u.name));

    let users: Vec<repo::ExternalUser> = existing.iter()
        .filter_map(|u| repo::ExternalUser::new(members[&u.name], u.length).ok())
        .collect();
    if users.len() != existing.len() {
        return Err(anyhow!("couldn't convert integers for external users"))
    }

    repos.imports.import(chat_id, &users).await?;

    let not_found = not_existing.into_iter()
        .map(|u| u.name)
        .collect();
    Ok(ImportResult {
        found: existing,
        not_found
    })
}

fn map_users(capture: Option<Captures>) -> Option<OriginalUser> {
    let pos = capture?;
    if let (Some(name), Some(length)) = (pos.name("name"), pos.name("length")) {
        let name = name.as_str().to_owned();
        let length = length.as_str().parse().ok()?;
        return Some(OriginalUser { name, length })
    }
    None
}
