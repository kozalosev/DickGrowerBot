use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::types::Message;
use crate::handlers::{HandlerResult, promo_activation_impl, PROMO_START_PARAM_PREFIX, reply_html};
use crate::{metrics, reply_html, repo};
use crate::domain::LanguageCode;
use crate::help::HelpContainer;

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase")]
pub enum StartCommands {
    Start(String),
}

#[tracing::instrument]
pub async fn start_cmd_handler(bot: Bot, msg: Message, cmd: StartCommands,
                               help: HelpContainer, repos: repo::Repositories) -> HandlerResult {
    let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
    let answer = if msg.from.as_ref().is_none() {
        log::warn!("The /start command was invoked without a FROM field for message: {:?}", msg);
        help.get_help_message(lang_code).to_owned()
    } else {
        match cmd {
            StartCommands::Start(promo_code) if promo_code.starts_with(PROMO_START_PARAM_PREFIX) => {
                metrics::CMD_PROMO.invoked_by_deeplink.inc();
                let user = msg.from.as_ref().expect("user must be present here");
                let encoded_promo_code = promo_code.strip_prefix(PROMO_START_PARAM_PREFIX)
                    .expect("promo start param prefix must be present here");
                let promo_code = decode_promo_code(encoded_promo_code)?;
                promo_activation_impl(repos.promo, user, &promo_code).await?
            }
            StartCommands::Start(_) => {
                metrics::CMD_START_COUNTER.inc();
                let username = teloxide::utils::html::escape(&msg.from.as_ref().unwrap().first_name);
                help.get_start_message(username, lang_code)
            }
        }
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

fn decode_promo_code(promo_code_base64: &str) -> anyhow::Result<String> {
    let bytes = URL_SAFE_NO_PAD.decode(promo_code_base64)?;
    let promo_code = String::from_utf8(bytes)?;
    Ok(promo_code)
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    // TODO: implement a separate domain type for promo codes
    #[test]
    fn test_encode_decode_promo_code() {
        let code = "TEST_CODE";
        let encoded = URL_SAFE_NO_PAD.encode(code);

        let decoded_bytes = URL_SAFE_NO_PAD.decode(encoded.as_bytes())
            .expect("couldn't decode the encoded promo code");
        let decoded_str = String::from_utf8(decoded_bytes)
            .expect("couldn't convert promo code to a string");

        assert_eq!(code, decoded_str);
    }
}
