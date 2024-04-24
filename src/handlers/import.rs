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
use crate::{metrics, repo};

pub const ORIGINAL_BOT_USERNAMES: [&str; 2] = ["pipisabot", "kraft28_bot"];

static TOP_LINE_REGEXP: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{1,3}((\. )|\|)(?<name>.+?)(\.{3})? — (?<length>\d+) см.")
        .expect("TOP_LINE_REGEXP is invalid")
});

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum ImportCommands {
    #[command(description = "import")]
    Import
}

#[cfg_attr(test, derive(PartialEq, Eq))]
enum OriginalBotKind {
    Pipisa,
    Kraft28,
}

impl OriginalBotKind {
    fn convert_name(&self, name: &str) -> String {
        match self {
            OriginalBotKind::Pipisa => {
                name.chars()
                    .take(13)
                    .collect()
            },
            OriginalBotKind::Kraft28 => name.to_owned()
        }
    }
}

impl TryFrom<&str> for OriginalBotKind {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim_start_matches('@') {
            "pipisabot" => Ok(OriginalBotKind::Pipisa),
            "kraft28_bot" => Ok(OriginalBotKind::Kraft28),
            _ => Err("Unknown OriginalBotKind".to_owned())
        }
    }
}

struct ParseResult(OriginalBotKind, String);

#[derive(strum_macros::Display)]
#[strum(serialize_all="snake_case")]
enum BeforeImportCheckErrors {
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
    metrics::CMD_IMPORT.invoked();
    let lang_code = ensure_lang_code(msg.from());
    let answer = match check_and_parse_message(&bot, &msg).await {
        Ok(parsed) => {
            match import_impl(&repos, msg.chat.id, parsed).await {
                Ok(r) => {
                    metrics::CMD_IMPORT.finished();
                    let imported = r.imported.into_iter()
                        .map(|u| t!("commands.import.result.line.imported", locale = &lang_code,
                            name = teloxide::utils::html::escape(&u.name),
                            length = u.length))
                        .collect::<Vec<String>>()
                        .join("\n");
                    let already_present = r.already_present.into_iter()
                        .map(|u| t!("commands.import.result.line.already_present", locale = &lang_code,
                            name = teloxide::utils::html::escape(&u.name),
                            length = u.length))
                        .collect::<Vec<String>>()
                        .join("\n");
                    let not_found = r.not_found.into_iter()
                        .map(|name| t!("commands.import.result.line.not_found", locale = &lang_code,
                            name = teloxide::utils::html::escape(&name)))
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
                        .map(|line| t!("commands.import.errors.invalid_lines.line", locale = &lang_code,
                            line = line))
                        .collect::<Vec<String>>()
                        .join("\n");
                    t!("commands.import.errors.invalid_lines.template", locale = &lang_code,
                        invalid_lines = invalid_lines)
                } else {
                    Err(e)?
                }
            }

        },
        Err(BeforeImportCheckErrors::Other(e)) => Err(e)?,
        Err(BeforeImportCheckErrors::NotReply) => t!("commands.import.errors.not_reply",
            locale = lang_code.as_str(),
            origin_bots = ORIGINAL_BOT_USERNAMES.map(|name| format!("@{name}")).join(", ")),
        Err(e) => t!(format!("commands.import.errors.{e}").as_str(),
            locale = lang_code.as_str()),
    };
    reply_html(bot, msg, answer).await?;
    Ok(())
}

async fn check_and_parse_message(bot: &Bot, msg: &Message) -> Result<ParseResult, BeforeImportCheckErrors> {
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
    reply.from().as_ref()
        .filter(|u| u.is_bot)
        .and_then(|u| u.username.as_ref())
        .filter(|name| ORIGINAL_BOT_USERNAMES.contains(&name.as_ref()))
        .and_then(|name| {
            let name = name.as_str().try_into()
                .map_err(|name| log::error!("couldn't convert name: {name}"))
                .ok();
            if let (Some(name), Some(text)) = (name, reply.text()) {
                Some(ParseResult(name, text.to_owned()))
            } else {
                None
            }
        })
}

async fn import_impl(repos: &repo::Repositories, chat_id: ChatId, parsed: ParseResult) -> anyhow::Result<ImportResult> {
    let chat_id_kind = chat_id.into();
    let members: HashMap<String, ChatMember> = repos.users.get_chat_members(&chat_id_kind)
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

    let imported_uids: HashSet<UserId> = repos.import.get_imported_users(chat_id)
        .await?.into_iter()
        .filter_map(|u| u.uid.try_into().ok())
        .map(UserId)
        .collect();

    let (already_present, to_import): (Vec<UserInfo>, Vec<UserInfo>) = existing.into_iter()
        .partition(|u| imported_uids.contains(&u.uid));

    let users: Vec<repo::ExternalUser> = to_import.iter()
        .map(|u| repo::ExternalUser::new(u.uid, u.length))
        .collect();
    if users.len() != to_import.len() {
        return Err(anyhow!("couldn't convert integers for external users"))
    }
    repos.import.import(chat_id, &users).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_bot_kind_convert_name_pipisa() {
        let (p, short) = (OriginalBotKind::Pipisa, "SadBot #incel".to_owned());
        assert_eq!(p.convert_name("SadBot #incel..."), short);
        assert_eq!(p.convert_name("SadBot #incel>suicide"), short);
    }

    #[test]
    fn original_bot_kind_try_from() {
        let check = |variant: &str, kind| {
            let second_variant = variant.strip_prefix('@').expect("no '@' prefix");
            let valid_variants = [variant, &second_variant];
            assert!(valid_variants.into_iter()
                .all(|v| OriginalBotKind::try_from(v)
                    .is_ok_and(|k| k == kind)));

            let invalid_variants = valid_variants.into_iter()
                .map(|v| {
                    v.strip_suffix("_bot")
                        .or_else (|| v.strip_suffix("bot"))
                        .expect("no 'bot' suffix")
                });
            assert!(invalid_variants.into_iter()
                .all(|v| OriginalBotKind::try_from(v)
                    .is_err()));
        } ;

        check("@pipisabot", OriginalBotKind::Pipisa);
        check("@kraft28_bot", OriginalBotKind::Kraft28);
    }
}
