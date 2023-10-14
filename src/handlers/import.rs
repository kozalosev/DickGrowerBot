use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
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

const ORIGINAL_BOT_USERNAMES: [&str; 2] = ["@pipisabot", "@kraft28_bot"];

static TOP_LINE_REGEXP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{1,3}((\. )|\|)(?<name>.+)(\.{3})? — (?<length>\d+) см.")
        .expect("TOP_LINE_REGEXP is invalid")
});

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum ImportCommands {
    Import
}

enum OriginalBotKind {
    PIPISA,
    KRAFT28,
}

impl OriginalBotKind {
    fn convert_name(&self, name: &str) -> String {
        match self {
            OriginalBotKind::PIPISA => {
                if name.len() > 13 {
                    &name[0..13]
                } else {
                    name
                }
            },
            OriginalBotKind::KRAFT28 => name
        }.to_owned()
    }
}

impl TryFrom<&String> for OriginalBotKind {
    type Error = String;

    fn try_from(value: &String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "@pipisa" => Ok(OriginalBotKind::PIPISA),
            "@kraft28_bot" => Ok(OriginalBotKind::KRAFT28),
            _ => Err("Unknown OriginalBotKind".to_owned())
        }
    }
}

struct ParseResult(OriginalBotKind, String);

#[derive(strum_macros::Display)]
#[strum(serialize_all="snake_case")]
enum BeforeImportCheckErrors {
    NotGroupChat,
    NotAdmin,
    NotReply,
    Other(anyhow::Error)
}

impl <T: Into<anyhow::Error>> From<T> for BeforeImportCheckErrors {
    fn from(value: T) -> Self {
        Self::Other(anyhow!(value))
    }
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

#[derive(Debug)]
struct InvalidLines(Vec<String>);

// Required only to wrap the error by anyhow!()
impl Display for InvalidLines {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let result = self.0.join(", ");
        f.write_str(&format!("[{result}]"))
    }
}

pub async fn import_cmd_handler(bot: Bot, msg: Message, repos: repo::Repositories) -> HandlerResult {
    let lang_code = ensure_lang_code(msg.from());
    let answer = match check_and_parse_message(&bot, &msg).await {
        Ok(parsed) => {
            match import_impl(&repos, msg.chat.id, parsed).await {
                Ok(r) => {
                    let imported = r.imported.into_iter()
                        .map(|u| t!("commands.import.result.line.imported",
                            name = u.name, length = u.length,
                            locale = &lang_code))
                        .collect::<Vec<String>>()
                        .join("\n");
                    let already_present = r.already_present.into_iter()
                        .map(|u| t!("commands.import.result.line.already_present",
                            name = u.name, length = u.length,
                            locale = &lang_code))
                        .collect::<Vec<String>>()
                        .join("\n");
                    let not_found = r.not_found.into_iter()
                        .map(|name| t!("commands.import.result.line.not_found",
                            name = name,
                            locale = &lang_code))
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
                Err(e) => if let Some(InvalidLines(lines)) = e.downcast_ref() {
                    log::error!("Invalid lines: {lines:?}");
                    let invalid_lines = lines.iter()
                        .map(|line| {
                            t!("commands.import.errors.invalid_lines.line",
                                line = line, locale = &lang_code)
                        })
                        .collect::<Vec<String>>()
                        .join("\n");
                    t!("commands.import.errors.invalid_lines.template",
                        invalid_lines = invalid_lines, locale = &lang_code)
                } else {
                    Err(e)?
                }
            }

        },
        Err(BeforeImportCheckErrors::Other(e)) => Err(e)?,
        Err(BeforeImportCheckErrors::NotReply) => t!(format!("commands.import.errors.not_reply").as_str(),
            origin_bots = ORIGINAL_BOT_USERNAMES.join(", "),
            locale = lang_code.as_str()),
        Err(e) => t!(format!("commands.import.errors.{e}").as_str(),
            locale = lang_code.as_str()),
    };
    reply_html(bot, msg, answer).await
}

async fn check_and_parse_message(bot: &Bot, msg: &Message) -> Result<ParseResult, BeforeImportCheckErrors> {
    if msg.chat.is_private() || msg.chat.is_channel() {
        return Err(BeforeImportCheckErrors::NotGroupChat)
    }

    let admin_ids = bot.get_chat_administrators(msg.chat.id)
        .await?
        .into_iter()
        .map(|m| m.user.id);
    let from_id = msg.from()
        .ok_or(BeforeImportCheckErrors::Other(anyhow!("not from a user")))?
        .id;
    let invoked_by_admin = admin_ids.into_iter().any(|id| id == from_id);
    if !invoked_by_admin {
        return Err(BeforeImportCheckErrors::NotAdmin)
    }

    let result = msg.reply_to_message()
        .filter(|m| m.forward().is_none())
        .and_then(check_reply_source_and_text);
    let result = match result {
        None => return Err(BeforeImportCheckErrors::NotReply),
        Some(res) => res
    };

    Ok(result)
}

fn check_reply_source_and_text(reply: &Message) -> Option<ParseResult> {
    reply.from().clone()
        .filter(|u| u.is_bot)
        .and_then(|u| u.username.as_ref())
        .filter(|name| ORIGINAL_BOT_USERNAMES.contains(&name.as_ref()))
        .and_then(|name| {
            if let (Some(name), Some(text)) = (name.try_into().ok(), reply.text()) {
                Some(ParseResult(name, text.to_owned()))
            } else {
                None
            }
        })
}

async fn import_impl(repos: &repo::Repositories, chat_id: ChatId, parsed: ParseResult) -> anyhow::Result<ImportResult> {
    let members: HashMap<String, ChatMember> = repos.users.get_chat_members(chat_id)
        .await?.into_iter()
        .map(|m| {
            let uid = m.uid.try_into().expect("couldn't convert uid to u64");
            let short_name = parsed.0.convert_name(&m.name);
            let member = ChatMember {
                uid: UserId(uid),
                full_name: m.name
            };
            (short_name, member)
        })
        .collect();
    let member_names: HashSet<_> = HashSet::from_iter(members.keys());

    let top: Vec<Result<Captures, String>> = parsed.1.lines()
        .skip_while(|s| !TOP_LINE_REGEXP.is_match(s))
        .map(|pos| TOP_LINE_REGEXP.captures(pos).ok_or(pos.to_owned()))
        .collect();
    let invalid_lines: Vec<String> = top.iter()
        .filter(|pos| pos.is_err())
        .map(|pos| pos.as_ref().unwrap_err().clone())
        .collect();
    if !invalid_lines.is_empty() {
        return Err(anyhow!(InvalidLines(invalid_lines)))
    }

    let top: Vec<OriginalUser> = top.into_iter()
        .filter(Result::is_ok)
        .map(Result::unwrap)
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

fn map_user(pos: Captures) -> Option<OriginalUser> {
    if let (Some(name), Some(length)) = (pos.name("name"), pos.name("length")) {
        let name = name.as_str().to_owned();
        let length = length.as_str().parse().ok()?;
        return Some(OriginalUser { name, length })
    }
    None
}
