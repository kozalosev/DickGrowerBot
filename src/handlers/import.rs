use std::collections::{HashMap, HashSet};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::requests::Requester;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, Me, Message, ReplyMarkup, UserId};
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

struct Params<'a> {
    bot: &'a Bot,
    msg: &'a Message,
    me: &'a Me,
    users: &'a repo::Users,
    dicks: &'a repo::Dicks,
    lang_code: &'a str,
}

pub async fn import_cmd_handler(bot: Bot, msg: Message, me: Me,
                             users: repo::Users, dicks: repo::Dicks) -> HandlerResult {
    let lang_code = ensure_lang_code(msg.from());
    let params = Params {
        bot: &bot,
        msg: &msg,
        me: &me,
        users: &users,
        dicks: &dicks,
        lang_code: &lang_code
    };
    let answer = if let Some(reply) = msg.reply_to_message() {
        if !check_reply_source(reply) {
            t!("", locale = p.lang_code)
        } else {
            if let Some(text) = reply.text() {
                import_impl(params, text).await?
            } else {
                t!("", locale = lang_code.as_str())
            }
        }
    } else {
        t!("commands.import.not_reply", locale = lang_code.as_str())
    };
    reply_html(bot, msg, answer).await
}

struct OriginalUser {
    name: String,
    length: u32
}

fn check_reply_source(reply: &Message) -> bool {
    reply.via_bot
        .and_then(|u| u.username)
        .filter(|n| n == ORIGINAL_BOT_USERNAME)
        .is_some()
}

async fn import_impl<'a>(p: Params<'a>, text: &str) -> anyhow::Result<String> {
    let chat_id = p.msg.chat.id;
    let members: HashMap<String, UserId> = p.users.get_chat_members(chat_id)
        .await?.into_iter()
        .map(|m| (m.name, UserId(m.uid.into())))
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

    let answer = if !not_existing.is_empty() {
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback(t!("", locale = p.lang_code), "import:yes"),
            InlineKeyboardButton::callback(t!("", locale = p.lang_code), "import:no")
        ]]);
        let answer = p.bot.send_message(chat_id, t!("", locale = p.lang_code));
        answer.reply_markup = Some(ReplyMarkup::InlineKeyboard(keyboard));
        answer
    } else {
        let users: Vec<repo::ExternalUser> = existing.iter()
            .map(|u| repo::ExternalUser::new(members[&u.name], u.length))
            .collect();
        p.dicks.import_dicks(chat_id, users).await?;
        p.bot.send_message(chat_id, t!("", locale = p.lang_code))
    };
    Ok("")
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
