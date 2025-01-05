use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use once_cell::sync::Lazy;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::macros::BotCommands;
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::prelude::{Dialogue, InlineQuery, Requester};
use teloxide::types::{InlineQueryResultsButton, InlineQueryResultsButtonKind, Message, User};
use crate::handlers::{HandlerResult, reply_html};
use crate::{metrics, reply_html, repo};
use crate::domain::LanguageCode;
use crate::repo::ActivationError;

pub(crate) const PROMO_START_PARAM_PREFIX: &str = "promo-";

static PROMO_CODE_FORMAT_REGEXP: Lazy<regex::Regex> = Lazy::new(||
    regex::Regex::new("^[a-zA-Z0-9_\\-]{4,16}$")
        .expect("promo code format regular expression must be valid")
);

#[derive(BotCommands, Clone, Debug)]
#[command(rename_rule = "lowercase")]
pub enum PromoCommands {
    #[command(description = "promo")]
    Promo(String),
}

#[derive(Clone, Debug, Default)]
pub enum PromoCommandState {
    #[default]
    Start,
    Requested,
}

pub type PromoCodeDialogue = Dialogue<PromoCommandState, InMemStorage<PromoCommandState>>;

#[tracing::instrument]
pub async fn promo_cmd_handler(bot: Bot, msg: Message, cmd: PromoCommands, dialogue: PromoCodeDialogue,
                               repos: repo::Repositories) -> HandlerResult {
    metrics::CMD_PROMO.invoked_by_command.inc();
    let user = msg.from.as_ref().ok_or("no from user")?;
    let answer = match cmd {
        PromoCommands::Promo(code) if code.is_empty() => {
            dialogue.update(PromoCommandState::Requested).await?;

            let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
            t!("commands.promo.request", locale = &lang_code).to_string()
        }
        PromoCommands::Promo(code) => {
            dialogue.exit().await?;
            
            promo_activation_impl(repos.promo, user, &code).await?
        },
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

#[tracing::instrument]
pub async fn promo_requested_handler(bot: Bot, msg: Message, dialogue: PromoCodeDialogue,
                                     repos: repo::Repositories) -> HandlerResult {
    let answer = match msg.text() {
        Some(code) => {
            dialogue.exit().await?;
            
            let user = msg.from.as_ref().ok_or("no from user")?;
            promo_activation_impl(repos.promo, user, code).await?
        },
        None => {
            let lang_code = LanguageCode::from_maybe_user(msg.from.as_ref());
            t!("commands.promo.request", locale = &lang_code).to_string()
        }
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

pub fn promo_inline_filter(InlineQuery { query, .. }: InlineQuery) -> bool {
    PROMO_CODE_FORMAT_REGEXP.is_match(&query)
}

#[tracing::instrument]
pub async fn promo_inline_handler(bot: Bot, query: InlineQuery) -> HandlerResult {
    metrics::INLINE_COUNTER.invoked();

    let lang_code = LanguageCode::from_user(&query.from);
    let promo_code = query.query;
    let button_text = t!("commands.promo.inline.switch_button", locale = &lang_code, code = promo_code);
    let encoded_query = URL_SAFE_NO_PAD.encode(promo_code.as_bytes());
    let deeplink_start_param = format!("{}{}", PROMO_START_PARAM_PREFIX, encoded_query);
    let button = InlineQueryResultsButton {
        text: button_text.to_string(),
        kind: InlineQueryResultsButtonKind::StartParameter(deeplink_start_param)
    };
    let mut answer = bot.answer_inline_query(query.id, Vec::default())
        .is_personal(true)
        .button(button);
    if cfg!(debug_assertions) {
        answer.cache_time.replace(1);
    }
    answer.await?;
    Ok(())
}

#[tracing::instrument]
pub(crate) async fn promo_activation_impl(promo_repo: repo::Promo, user: &User, promo_code: &str) -> anyhow::Result<String> {
    let lang_code = LanguageCode::from_user(user);
    let answer = match promo_repo.activate(user.id, promo_code).await {
        Ok(res) => {
            metrics::CMD_PROMO.finished.inc();
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
                .to_string()
        },
        Err(e) => {
            let suffix = match e {
                ActivationError::Other(e) => Err(e)?,
                e => format!("{e}")
            };
            let t_key = format!("commands.promo.errors.{suffix}");
            t!(&t_key, locale = &lang_code).to_string()
        }
    };
    Ok(answer)
}

fn get_chats_in_russian(count: u64) -> String {
    match count {
        1 => "чате",
        _ => "чатах"
    }.to_owned()
}


#[cfg(test)]
mod test {
    use crate::handlers::promo::PROMO_CODE_FORMAT_REGEXP;

    #[test]
    fn test_regex() {
        assert!(PROMO_CODE_FORMAT_REGEXP.is_match("TESTPROMO"));
        assert!(PROMO_CODE_FORMAT_REGEXP.is_match("test-11_1"));

        assert!(!PROMO_CODE_FORMAT_REGEXP.is_match("T34"));
        assert!(!PROMO_CODE_FORMAT_REGEXP.is_match("PROMO!"));
        assert!(!PROMO_CODE_FORMAT_REGEXP.is_match("VERYVERYLONGLONGPROMOCODE"));
    }
}
