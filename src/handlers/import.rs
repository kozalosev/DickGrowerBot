use std::collections::{HashMap, HashSet};
use once_cell::sync::Lazy;
use strum_macros::Display;
use regex::{Captures, Regex};
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, ReplyMarkup, UserId};
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::repo;

const ORIGINAL_BOT_USERNAME: &str = "@pipisabot";

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

struct Repositories<'a> {
    users: &'a repo::Users,
    dicks: &'a repo::Dicks,
    imports: &'a repo::Imports,
}

struct Params<'a> {
    bot: &'a Bot,
    msg: &'a Message,
    lang_code: &'a str,
    repos: Repositories<'a>,
}

impl <'a> Params<'a> {
    fn chat_id(&self) -> ChatId {
        self.msg.chat.id
    }
}

#[derive(Display)]
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

pub async fn import_cmd_handler(bot: Bot, msg: Message,
                                users: repo::Users, dicks: repo::Dicks,
                                imports: repo::Imports) -> HandlerResult {
    let lang_code = ensure_lang_code(msg.from());
    let params = Params {
        bot: &bot,
        msg: &msg,
        repos: Repositories {
            users: &users,
            dicks: &dicks,
            imports: &imports,
        },
        lang_code: &lang_code
    };

    let answer = match check_params_and_parse_message(&params).await {
        Ok(txt) => import_impl(&params, txt).await?,
        Err(e) => t!(format!("commands.import.errors.{e}").as_str(), locale = lang_code.as_str()),
    };
    reply_html(bot, msg, answer).await
}

async fn check_params_and_parse_message<'a>(p: &Params<'a>) -> Result<String, BeforeImportCheckErrors> {
    let admin_ids = p.bot.get_chat_administrators(p.chat_id())
        .await?.into_iter()
        .map(|m| m.user.id);
    let from_id = p.msg.from().ok_or("not from a user")?.id;
    let invoked_by_admin = admin_ids.into_iter().any(|id| id == from_id);
    if !invoked_by_admin {
        return Err(BeforeImportCheckErrors::NotAdmin)
    }

    let text = p.msg.reply_to_message()
        .filter(check_reply_source)
        .and_then(|reply| reply.text());
    let text = match text {
        None => return Err(BeforeImportCheckErrors::NotReply),
        Some(text) => text.to_owned()
    };

    let already_imported = p.repos.imports.were_dicks_already_imported(p.chat_id()).await?;
    if already_imported {
        return Err(BeforeImportCheckErrors::AlreadyImported)
    }

    Ok(text)
}

struct OriginalUser {
    name: String,
    length: u32
}

fn check_reply_source(reply: &&Message) -> bool {
    reply.via_bot
        .and_then(|u| u.username)
        .filter(|n| n == ORIGINAL_BOT_USERNAME)
        .is_some()
}

async fn import_impl<'a>(p: &Params<'a>, text: String) -> anyhow::Result<String> {
    let members: HashMap<String, UserId> = p.repos.users.get_chat_members(p.chat_id())
        .await?.into_iter()
        .map(|m| {
            let uid = m.uid.try_into().expect("couldn't convert uid to u64");
            (m.name, UserId(uid))
        })
        .collect();
    let member_names = HashSet::from_iter(members.keys());  // TODO: cloned()?

    let top: Vec<Option<Captures>> = text.lines()
        .skip(2)
        .map(|pos| TOP_LINE_REGEXP.captures(pos))
        .collect();
    let invalid_lines = top.iter()
        .filter(|pos| pos.is_none())
        .collect();
    let top: Vec<OriginalUser> = top.iter()
        .filter_map(map_users)
        .collect();

    let (existing, not_existing): (Vec<OriginalUser>, Vec<OriginalUser>) = top.into_iter()
        .partition(|u| member_names.contains(&u.name));

    if !not_existing.is_empty() {
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback("👍🏼", "import:yes"),
            InlineKeyboardButton::callback("👎🏼", "import:no")
        ]]);
        let answer = p.bot.send_message(p.chat_id(), t!("commands.import.confirmation",
            locale = p.lang_code));
        answer.reply_markup = Some(ReplyMarkup::InlineKeyboard(keyboard));
        answer.await?
    } else {
        let users: Vec<repo::ExternalUser> = existing.iter()
            .map(|u| repo::ExternalUser::new(members[&u.name], u.length))
            .collect();
        p.repos.dicks.import_dicks(p.chat_id(), users).await?;
        p.bot.send_message(p.chat_id(), t!("commands.import.result", locale = p.lang_code)).await?
    };
    Ok(())
}

fn map_users(capture: &Option<Captures>) -> Option<OriginalUser> {
    let pos = (*capture)?;
    if let (Some(name), Some(length)) = (pos.name("name"), pos.name("length")) {
        let name = name.as_str().to_owned();
        let length = length.as_str().parse().ok()?;
        return Some(OriginalUser { name, length })
    }
    None
}
