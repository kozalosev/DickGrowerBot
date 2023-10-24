use std::ops::RangeInclusive;
use anyhow::anyhow;
use chrono::Utc;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html, utils};
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
            let user = repos.users.create_or_update(from.id, name).await?;
            let days_since_registration = (Utc::now() - user.created_at).num_days() as u32;
            let grow_shrink_ratio = if days_since_registration > config.newcomers_grace_days {
                config.grow_shrink_ratio
            } else {
                1.0
            };
            let increment = gen_increment(config.growth_range, grow_shrink_ratio);
            let grow_result = repos.dicks.create_or_grow(from.id, msg.chat.id, increment).await;
            let lang_code = ensure_lang_code(Some(from));

            let main_part = match grow_result {
                Ok(repo::GrowthResult { new_length, pos_in_top }) => {
                    t!("commands.grow.result", locale = &lang_code,
                        incr = increment, length = new_length, pos = pos_in_top)
                },
                Err(e) => {
                    let db_err = e.downcast::<sqlx::Error>()?;
                    if let sqlx::Error::Database(e) = db_err {
                        e.code()
                            .filter(|c| c == TOMORROW_SQL_CODE)
                            .map(|_| t!("commands.grow.tomorrow", locale = &lang_code))
                            .ok_or(anyhow!(e))?
                    } else {
                        Err(db_err)?
                    }
                }
            };
            let time_left_part = utils::date::get_time_till_next_day_string(&lang_code);
            format!("{main_part}{time_left_part}")
        },
        DickCommands::Grow => Err("unexpected absence of a FROM field for the /grow command")?,
        DickCommands::Top => {
            metrics::CMD_TOP_COUNTER.inc();

            let lang_code = ensure_lang_code(msg.from());
            let lines = repos.dicks.get_top(msg.chat.id, config.top_limit)
                .await?
                .iter().enumerate()
                .map(|(i, d)| {
                    let name = teloxide::utils::html::escape(&d.owner_name);
                    let can_grow = (chrono::Utc::now() - d.grown_at).num_days() > 0;
                    let line = t!("commands.top.line", locale = &lang_code,
                        n = i+1, name = name, length = d.length);
                    if can_grow {
                        format!("{line} [+]")
                    } else {
                        line
                    }
                })
                .collect::<Vec<String>>();

            if lines.is_empty() {
                t!("commands.top.empty", locale = &lang_code)
            } else {
                let title = t!("commands.top.title", locale = &lang_code);
                let ending = t!("commands.top.ending", locale = &lang_code);
                format!("{}\n\n{}\n\n{}", title, lines.join("\n"), ending)
            }
        }
    };
    reply_html(bot, msg, answer).await
}

fn gen_increment(range: RangeInclusive<i32>, sign_ratio: f32) -> i32 {
    let sign_ratio_percent = match (sign_ratio * 100.0).round() as u32 {
        ..=0 => 0,
        100.. => 100,
        x => x
    };
    let mut rng = OsRng::default();
    let positive = rng.gen_ratio(sign_ratio_percent, 100);
    let end = if positive {
        *range.end()
    } else {
        range.start().abs()
    };
    rng.gen_range(1..=end)
}
