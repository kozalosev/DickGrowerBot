use std::borrow::Cow;
use anyhow::anyhow;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::{Message, UserId};
use crate::{config, metrics, repo};
use crate::handlers::{ensure_lang_code, FromRefs, HandlerResult, reply_html, utils};

const DOD_ALREADY_CHOSEN_SQL_CODE: &str = "GD0E2";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum DickOfDayCommands {
    DickOfDay,
    Dod,
}

pub async fn dod_cmd_handler(bot: Bot, msg: Message,
                             repos: repo::Repositories, config: config::AppConfig) -> HandlerResult {
    metrics::CMD_DOD_COUNTER.chat.inc();
    let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);
    let answer = dick_of_day_impl(&repos, config, from_refs).await?;
    reply_html(bot, msg, answer).await
}

pub(crate) async fn dick_of_day_impl(repos: &repo::Repositories, config: config::AppConfig, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
    let (from, chat_id) = (from_refs.0, from_refs.1.into());
    let lang_code = ensure_lang_code(Some(from));
    let winner = repos.users.get_random_active_member(chat_id).await?;
    let answer = match winner {
        Some(winner) => {
            let bonus: u32 = OsRng::default().gen_range(config.dod_bonus_range);
            let dod_result = repos.dicks.set_dod_winner(&chat_id, UserId(winner.uid as u64), bonus).await;
            let main_part = match dod_result {
                Ok(repo::GrowthResult{ new_length, pos_in_top }) => {
                    t!("commands.dod.result", locale = &lang_code,
                        name = winner.name, growth = bonus, length = new_length, pos = pos_in_top)
                },
                Err(e) => {
                    match e.downcast::<sqlx::Error>()? {
                        sqlx::Error::Database(e)
                        if e.code() == Some(Cow::Borrowed(DOD_ALREADY_CHOSEN_SQL_CODE)) => {
                            t!("commands.dod.already_chosen", locale = &lang_code, name = e.message())
                        }
                        e => Err(e)?
                    }
                }
            };
            let time_left_part = utils::date::get_time_till_next_day_string(&lang_code);
            format!("{main_part}{time_left_part}")
        },
        None => t!("commands.dod.no_candidates", locale = &lang_code)
    };
    Ok(answer)
}
