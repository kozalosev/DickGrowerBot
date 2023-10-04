use anyhow::anyhow;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::{config, metrics, repo};

const TOMORROW_SQL_CODE: &str = "GD0E1";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum DickCommands {
    Grow,
    Top,
}

pub async fn dick_cmd_handler(bot: Bot, msg: Message, cmd: DickCommands,
                              users: repo::Users, dicks: repo::Dicks,
                              config: config::AppConfig) -> HandlerResult {
    let answer = match cmd {
        DickCommands::Grow if msg.from().is_some() => {
            metrics::CMD_GROW_COUNTER.inc();

            let from = msg.from().unwrap();
            let name = from.last_name.as_ref()
                .map(|last_name| format!("{} {}", from.first_name, last_name))
                .unwrap_or(from.first_name.clone());
            let increment = OsRng::default().gen_range(config.growth_range);

            users.create_or_update(from.id, name).await?;
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
