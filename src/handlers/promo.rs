use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html};
use crate::{metrics, repo};
use crate::repo::ActivationError;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum PromoCommands {
    Promo(String)
}

pub async fn promo_cmd_handler(bot: Bot, msg: Message, cmd: PromoCommands,
                               repos: repo::Repositories) -> HandlerResult {
    metrics::CMD_PROMO.invoked();
    let PromoCommands::Promo(code) = cmd;
    let user = msg.from().ok_or("no from user")?;
    let lang_code = ensure_lang_code(msg.from());

    let answer = match repos.promo.activate(user.id, &code).await {
        Ok(res) => {
            metrics::CMD_PROMO.finished();
            let suffix = if res.chats_affected > 1 {
                "plural"
            } else {
                "singular"
            };
            let chats_in_russian = get_chats_in_russian(res.chats_affected);
            t!("commands.promo.success.template", locale = &lang_code,
                ending = t!(&format!("commands.promo.success.{suffix}"), locale = &lang_code,
                    growth = res.bonus_length, affected_chats = res.chats_affected,
                    word_chats = chats_in_russian))
        },
        Err(e) => {
            let suffix = match e {
                ActivationError::Other(e) => Err(e)?,
                e => format!("{e}")
            };
            t!(&format!("commands.promo.errors.{suffix}"), locale = &lang_code)
        }
    };
    reply_html(bot, msg, answer).await
}

fn get_chats_in_russian(count: u64) -> String {
    match count % 10 {
        1 if count != 11 => "чат",
        2..=4 if !(12..=14).contains(&count) => "чата",
        _ => "чатов"
    }.to_owned()
}
