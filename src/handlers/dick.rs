use std::ops::RangeInclusive;
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
                              repos: repo::Repositories, config: config::AppConfig) -> HandlerResult {
    let answer = match cmd {
        DickCommands::Grow if msg.from().is_some() => {
            metrics::CMD_GROW_COUNTER.inc();

            let from = msg.from().unwrap();
            let name = from.last_name.as_ref()
                .map(|last_name| format!("{} {}", from.first_name, last_name))
                .unwrap_or(from.first_name.clone());
            let increment = gen_increment(config.growth_range, config.grow_shrink_ratio);

            repos.users.create_or_update(from.id, name).await?;
            let grow_result = repos.dicks.create_or_grow(from.id, msg.chat.id, increment).await;

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
            let lines = repos.dicks.get_top(msg.chat.id)
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

fn gen_increment(range: RangeInclusive<i32>, positive_distribution_coef: f32) -> i32 {
    let min_max_ratio = {
        let start_abs = range.start().abs() as f32;
        let end = *range.end() as f32;
        start_abs / (start_abs + end)
    };
    let mut rng = OsRng::default();
    let positive = rng.gen_range(0.0..=positive_distribution_coef) > min_max_ratio;
    let end = if positive {
        *range.end()
    } else {
        range.start().abs()
    };
    rng.gen_range(1..=end)
}
