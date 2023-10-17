use std::borrow::Cow;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::{config, metrics, repo};
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};

const DOD_ALREADY_CHOSEN_SQL_CODE: &str = "GD0E2";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
pub enum DickOfDayCommands {
    DickOfDay,
    Dod,
}

pub async fn dod_cmd_handler(bot: Bot, msg: Message,
                             repos: repo::Repositories, config: config::AppConfig) -> HandlerResult {
    metrics::CMD_DOD_COUNTER.inc();
    let chat_id = msg.chat.id;
    let winner = repos.users.get_random_active_member(chat_id).await?;
    let bonus: u32 = OsRng::default().gen_range(config.dod_bonus_range);
    let dod_result = repos.dicks.set_dod_winner(chat_id, repo::UID(winner.uid), bonus).await;
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
