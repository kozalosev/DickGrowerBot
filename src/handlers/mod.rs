use std::borrow::Cow;
use anyhow::anyhow;
use rand::{Rng, rngs::OsRng};
use teloxide::utils::command::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{Me, User};
use teloxide::types::ParseMode::Html;
use rust_i18n::t;
use crate::{config, help, repo};
use crate::metrics;

const TOMORROW_SQL_CODE: &str = "GD0E1";
const DOD_ALREADY_CHOSEN_SQL_CODE: &str = "GD0E2";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum HelpCommands {
    Start,
    Help,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum DickCommands {
    Grow,
    Top,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum DickOfDayCommands {
    DickOfDay
}

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

pub async fn help_cmd_handler(bot: Bot, msg: Message, cmd: HelpCommands, me: Me) -> HandlerResult {
    let help = match cmd {
        HelpCommands::Start if msg.from().is_some() => {
            metrics::CMD_START_COUNTER.inc();
            help::get_start_message(msg.from().unwrap(), me)
        },
        HelpCommands::Help => {
            metrics::CMD_HELP_COUNTER.inc();
            help::get_help_message(msg.from(), me)
        }
        HelpCommands::Start => {
            log::warn!("The /start or /help command was invoked without a FROM field for message: {:?}", msg);
            help::get_help_message(msg.from(), me)
        },
    };

    bot.send_message(msg.chat.id, help).await?;
    Ok(())
}

pub async fn dick_cmd_handler(bot: Bot, msg: Message, cmd: DickCommands,
                              users: repo::Users, dicks: repo::Dicks,
                              config: config::AppConfig) -> HandlerResult {
    let answer = match cmd {
        DickCommands::Grow if msg.from().is_some() => {
            metrics::CMD_GROW_COUNTER.inc();

            let from = msg.from().unwrap();
            let increment = OsRng::default().gen_range(config.growth_range);

            users.create_or_update(from.id, from.first_name.clone()).await?;
            let grow_result = dicks.create_or_grow(from.id, msg.chat.id, increment).await;

            match grow_result {
                Ok(new_length) => {
                    let lang_code = ensure_lang_code(Some(from));
                    t!("commands.grow.result", locale = lang_code.as_str(), incr = increment, length = new_length)
                },
                Err(e) => {
                    let db_err = e.downcast::<sqlx::Error>()?;
                    if let sqlx::Error::Database(e) = db_err {
                        e.code()
                            .filter(|c| c == TOMORROW_SQL_CODE)
                            .map(|_| t!("commands.grow.tomorrow"))
                            .ok_or(anyhow!(e))?
                    } else {
                        Err(db_err)?
                    }
                }
            }
        },
        DickCommands::Grow => Err("unexpected absence of a FROM field for the /grow command")?,
        DickCommands::Top => {
            metrics::CMD_TOP_COUNTER.inc();

            let lang_code = ensure_lang_code(msg.from());
            let title = t!("commands.top.title", locale = lang_code.as_str());
            let lines = dicks.get_top(msg.chat.id)
                .await?
                .iter().enumerate()
                .map(|(i, d)| {
                    let name = teloxide::utils::html::escape(d.owner_name.as_str());
                    t!("commands.top.line", locale = lang_code.as_str(), n = i+1, name = name, length = d.length)
                })
                .collect::<Vec<String>>();

            if lines.is_empty() {
                t!("commands.top.empty", locale = lang_code.as_str())
            } else {
                format!("{}\n\n{}", title, lines.join("\n"))
            }
        }
    };
    reply_html(bot, msg, answer).await
}

pub async fn dod_cmd_handler(bot: Bot, msg: Message,
                             users: repo::Users, dicks: repo::Dicks,
                             config: config::AppConfig) -> HandlerResult {
    let chat_id = msg.chat.id;
    let winner = users.get_random_active_member(chat_id).await?;
    let bonus: u32 = OsRng::default().gen_range(config.dod_bonus_range);
    let dod_result = dicks.set_dod_winner(chat_id, repo::UID(winner.uid), bonus).await;
    let lang_code = ensure_lang_code(msg.from());

    let answer = match dod_result {
        Ok(new_length) => t!("commands.dod.result", locale = lang_code.as_str(),
            name = winner.name, growth = bonus, length = new_length),
        Err(e) => {
            match e.downcast::<sqlx::Error>()? {
                sqlx::Error::Database(e)
                if e.code() == Some(Cow::Borrowed(DOD_ALREADY_CHOSEN_SQL_CODE)) => {
                    t!("commands.dod.already_chosen", locale = lang_code.as_str(), name = e.message())
                }
                e => Err(e)?
            }
        }
    };
    reply_html(bot, msg, answer).await
}

pub fn ensure_lang_code(user: Option<&User>) -> String {
    user
        .map(|u| {
            u.language_code.clone()
                .or_else(|| {
                    log::warn!("no language_code for {}, using the default", u.id);
                    None
                })
        })
        .flatten()
        .unwrap_or("en".to_owned())
}

async fn reply_html(bot: Bot, msg: Message, answer: String) -> HandlerResult {
    let mut answer = bot.send_message(msg.chat.id, answer);
    answer.parse_mode = Some(Html);
    answer.await?;
    Ok(())
}
