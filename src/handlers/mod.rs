use rand::{Rng, rngs::OsRng};
use sqlx::{Pool, Postgres, Row};
use teloxide::utils::command::BotCommands;
use teloxide::prelude::*;
use teloxide::types::Me;
use teloxide::types::ParseMode::Html;
use rust_i18n::t;
use crate::help;
use crate::metrics;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum Command {
    Start,
    Help,
    Grow,
    Top,
}

pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

pub async fn command_handler(bot: Bot, msg: Message, cmd: Command, me: Me, pool: Pool<Postgres>) -> HandlerResult {
    let help = match cmd {
        Command::Start if msg.from().is_some() => {
            metrics::CMD_START_COUNTER.inc();
            help::get_start_message(msg.from().unwrap(), me)
        },
        Command::Help => {
            metrics::CMD_HELP_COUNTER.inc();
            help::get_help_message(msg.from(), me)
        }
        Command::Start => {
            log::warn!("The /start or /help command was invoked without a FROM field for message: {:?}", msg);
            help::get_help_message(msg.from(), me)
        }
        Command::Grow if msg.from().is_some() => {
            metrics::CMD_GROW_COUNTER.inc();

            let from = msg.from().unwrap();
            let (UserId(uid), ChatId(chat_id)) = (from.id, msg.chat.id);
            let uid: i64 = uid.try_into()?;

            sqlx::query("INSERT INTO Users(uid, name) VALUES ($1, $2) ON CONFLICT (uid) DO UPDATE SET name = $2")
                .bind(uid)
                .bind(from.first_name.clone())
                .execute(&pool).await?;

            let increment = OsRng::default().gen_range(-5..=10);
            let new_length: i32 = sqlx::query("INSERT INTO dicks(uid, chat_id, length) VALUES ($1, $2, $3) ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $3) RETURNING length")
                .bind(uid)
                .bind(chat_id)
                .bind(increment)
                .fetch_one(&pool).await?
                .try_get("length")?;

            let lang_code = from.language_code.clone().unwrap_or("en".to_string());
            t!("commands.grow.result", locale = lang_code.as_str(), incr = increment, length = new_length)
        },
        Command::Grow => Err("unexpected absence of a FROM field for the /grow command")?,
        Command::Top => {
            metrics::CMD_TOP_COUNTER.inc();

            let lang_code = msg.from()
                .map(|u| u.language_code.clone())
                .flatten()
                .unwrap_or("en".to_string());
            let title = t!("commands.top.title", locale = lang_code.as_str());
            let lines = sqlx::query_as::<_, Dick>("SELECT length, name as owner_name FROM dicks d JOIN users u ON u.uid = d.uid WHERE chat_id = $1 ORDER BY length DESC LIMIT 10")
                .bind(msg.chat.id.0)
                .fetch_all(&pool)
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

    let mut answer = bot.send_message(msg.chat.id, help);
    answer.parse_mode = Some(Html);
    answer.await?;
    Ok(())
}

#[derive(sqlx::FromRow, Debug)]
struct Dick {
    length: i32,
    owner_name: String,
}
