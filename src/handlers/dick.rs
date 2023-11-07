use std::ops::RangeInclusive;
use anyhow::anyhow;
use chrono::{Datelike, Utc};
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::{Message, User};
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html, utils};
use crate::{config, metrics, repo};
use crate::repo::ChatIdKind;

const TOMORROW_SQL_CODE: &str = "GD0E1";
const LTR_MARK: char = '\u{200E}';

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum DickCommands {
    #[command(description = "grow")]
    Grow,
    #[command(description = "top")]
    Top,
}

pub async fn dick_cmd_handler(bot: Bot, msg: Message, cmd: DickCommands,
                              repos: repo::Repositories, config: config::AppConfig) -> HandlerResult {
    let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);
    let answer = match cmd {
        DickCommands::Grow => {
            metrics::CMD_GROW_COUNTER.chat.inc();
            grow_impl(&repos, config, from_refs).await?
        },
        DickCommands::Top => {
            metrics::CMD_TOP_COUNTER.chat.inc();
            top_impl(&repos, config, from_refs).await?
        }
    };
    reply_html(bot, msg, answer).await
}

pub struct FromRefs<'a>(pub &'a User, pub &'a ChatIdKind);

pub(crate) async fn grow_impl(repos: &repo::Repositories, config: config::AppConfig, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
    let (from, chat_id) = (from_refs.0, from_refs.1.into());
    let name = utils::get_full_name(from);
    let user = repos.users.create_or_update(from.id, &name).await?;
    let days_since_registration = (Utc::now() - user.created_at).num_days() as u32;
    let grow_shrink_ratio = if days_since_registration > config.newcomers_grace_days {
        config.grow_shrink_ratio
    } else {
        1.0
    };
    let increment = gen_increment(config.growth_range, grow_shrink_ratio);
    let grow_result = repos.dicks.create_or_grow(from.id, chat_id, increment).await;
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
    Ok(format!("{main_part}{time_left_part}"))
}

pub(crate) async fn top_impl(repos: &repo::Repositories, config: config::AppConfig, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
    let (from, chat_id) = (from_refs.0, from_refs.1.into());
    let lang_code = ensure_lang_code(Some(from));
    let lines = repos.dicks.get_top(chat_id, config.top_limit)
        .await?
        .iter().enumerate()
        .map(|(i, d)| {
            let ltr_name = format!("{LTR_MARK}{}{LTR_MARK}", d.owner_name);
            let name = teloxide::utils::html::escape(&ltr_name);
            let can_grow = chrono::Utc::now().num_days_from_ce() > d.grown_at.num_days_from_ce();
            let line = t!("commands.top.line", locale = &lang_code,
                n = i+1, name = name, length = d.length);
            if can_grow {
                format!("{line} [+]")
            } else {
                line
            }
        })
        .collect::<Vec<String>>();

    let res = if lines.is_empty() {
        t!("commands.top.empty", locale = &lang_code)
    } else {
        let title = t!("commands.top.title", locale = &lang_code);
        let ending = t!("commands.top.ending", locale = &lang_code);
        format!("{}\n\n{}\n\n{}", title, lines.join("\n"), ending)
    };
    Ok(res)
}

fn gen_increment(range: RangeInclusive<i32>, sign_ratio: f32) -> i32 {
    let sign_ratio_percent = match (sign_ratio * 100.0).round() as u32 {
        ..=0 => 0,
        100.. => 100,
        x => x
    };
    let mut rng = OsRng::default();
    if range.start() > &0 {
        return rng.gen_range(range)
    }
    let positive = rng.gen_ratio(sign_ratio_percent, 100);
    if positive {
        let end = *range.end();
        rng.gen_range(1..=end)
    } else {
        let start = *range.start();
        rng.gen_range(start..=-1)
    }

}

#[cfg(test)]
mod test {
    use super::gen_increment;

    #[test]
    fn test_gen_increment() {
        let increments: Vec<i32> = (0..100)
            .map(|_| gen_increment(-5..=10, 0.5))
            .collect();
        assert!(increments.iter().any(|n| n > &0));
        assert!(increments.iter().any(|n| n < &0));
        assert!(increments.iter().all(|n| n != &0));
        assert!(increments.iter().all(|n| n <= &10));
        assert!(increments.iter().all(|n| n >= &-5));
    }

    #[test]
    fn test_gen_increment_with_positive_range() {
        let increments: Vec<i32> = (0..100)
            .map(|_| gen_increment(5..=10, 0.5))
            .collect();
        assert!(increments.iter().all(|n| n <= &10));
        assert!(increments.iter().all(|n| n >= &5));
    }
}
