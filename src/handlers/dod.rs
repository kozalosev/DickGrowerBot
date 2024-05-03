use std::borrow::Cow;
use anyhow::anyhow;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::{Message, UserId};
use crate::{metrics, repo};
use crate::handlers::{ensure_lang_code, FromRefs, HandlerResult, reply_html, utils};
use crate::handlers::utils::Incrementor;

const DOD_ALREADY_CHOSEN_SQL_CODE: &str = "GD0E2";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum DickOfDayCommands {
    #[command(description = "dod")]
    DickOfDay,
    Dod,
}

pub async fn dod_cmd_handler(bot: Bot, msg: Message,
                             repos: repo::Repositories, incr: Incrementor) -> HandlerResult {
    metrics::CMD_DOD_COUNTER.chat.inc();
    let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);
    let answer = dick_of_day_impl(&repos, incr, from_refs).await?;
    reply_html(bot, msg, answer).await?;
    Ok(())
}

pub(crate) async fn dick_of_day_impl(repos: &repo::Repositories, incr: Incrementor, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
    let (from, chat_id) = (from_refs.0, from_refs.1);
    let lang_code = ensure_lang_code(Some(from));
    let winner = repos.users.get_random_active_member(&chat_id.kind()).await?;
    let answer = match winner {
        Some(winner) => {
            let increment = incr.dod_increment(from.id, chat_id.kind()).await;
            let dod_result = repos.dicks.set_dod_winner(chat_id, UserId(winner.uid as u64), increment.total).await;
            let main_part = match dod_result {
                Ok(Some(repo::GrowthResult{ new_length, pos_in_top })) => {
                    let answer = t!("commands.dod.result", locale = &lang_code,
                        name = winner.name, growth = increment.total, length = new_length);
                    let perks_part = increment.perks_part_of_answer(&lang_code);
                    if let Some(pos) = pos_in_top {
                        let position = t!("commands.dod.position", locale = &lang_code, pos = pos);
                        format!("{answer}\n{position}{perks_part}")
                    } else {
                        format!("{answer}{perks_part}")
                    }
                },
                Ok(None) => {
                    log::error!("there was an attempt to set a non-existent dick as a winner (UserID={}, ChatId={})",
                        winner.uid, chat_id);
                    t!("commands.dod.no_candidates", locale = &lang_code)
                }
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
