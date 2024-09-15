use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::handlers::{HandlerResult, promo_activation_impl, PROMO_START_PARAM_PREFIX, reply_html};
use crate::{metrics, repo};
use crate::domain::LanguageCode;
use crate::help::HelpContainer;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum StartCommands {
    Start(String),
}

pub async fn start_cmd_handler(bot: Bot, msg: Message, cmd: StartCommands,
                               help: HelpContainer, repos: repo::Repositories) -> HandlerResult {
    let lang_code = LanguageCode::from_maybe_user(msg.from());
    let answer = if msg.from().is_none() {
        log::warn!("The /start command was invoked without a FROM field for message: {:?}", msg);
        help.get_help_message(lang_code).to_owned()
    } else {
        match cmd {
            StartCommands::Start(promo_code) if promo_code.starts_with(PROMO_START_PARAM_PREFIX) => {
                metrics::CMD_PROMO.invoked_by_deeplink.inc();
                let user = msg.from().expect("user must be present here");
                let encoded_promo_code = promo_code.strip_prefix(PROMO_START_PARAM_PREFIX)
                    .expect("promo start param prefix must be present here");
                let promo_code = decode_promo_code(encoded_promo_code)?;
                promo_activation_impl(repos.promo, user, &promo_code).await?
            }
            StartCommands::Start(_) => {
                metrics::CMD_START_COUNTER.inc();
                let username = teloxide::utils::html::escape(&msg.from().unwrap().first_name);
                help.get_start_message(username, lang_code)
            }
        }
    };
    reply_html(bot, msg, answer).await?;
    Ok(())
}

fn decode_promo_code(promo_code_base64: &str) -> anyhow::Result<String> {
    let bytes = base64::decode_engine(promo_code_base64, &base64::engine::general_purpose::URL_SAFE_NO_PAD)?;
    let promo_code = String::from_utf8(bytes)?;
    Ok(promo_code)
}
