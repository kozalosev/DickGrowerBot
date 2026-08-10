use autometrics::autometrics;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::macros::BotCommands;
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::prelude::{Dialogue, InlineQuery, Requester};
use teloxide::types::{InlineQueryResultsButton, InlineQueryResultsButtonKind, Message, User};
use crate::handlers::{HandlerDeps, HandlerResult, reply_html};
use crate::{metrics, reply_html, repo};
use crate::domain::primitives::{AffectedRows, LanguageCode, PromoCode, UserId};
use crate::repo::ActivationError;

pub(crate) const PROMO_START_PARAM_PREFIX: &str = "promo-";

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum PromoCommands {
    #[command(description = "promo")]
    Promo(String),
}

#[derive(Clone, Default)]
pub enum PromoCommandState {
    #[default]
    Start,
    Requested,
}

pub type PromoCodeDialogue = Dialogue<PromoCommandState, InMemStorage<PromoCommandState>>;

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn promo_cmd_handler(
    bot: Bot,
    msg: Message,
    cmd: PromoCommands,
    dialogue: PromoCodeDialogue,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_PROMO.invoked_by_command.inc();
    let user = msg.from.as_ref().ok_or("no from user")?;
    let answer = match cmd {
        PromoCommands::Promo(code) if code.is_empty() => {
            dialogue.update(PromoCommandState::Requested).await?;

            t!("commands.promo.request", locale = &lang_code).to_string()
        }
        PromoCommands::Promo(code) => {
            dialogue.exit().await?;

            activate_or_refuse(repos.promo, user, code, lang_code).await?
        },
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn promo_requested_handler(
    bot: Bot,
    msg: Message,
    dialogue: PromoCodeDialogue,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let answer = match msg.text() {
        Some(code) => {
            dialogue.exit().await?;

            let user = msg.from.as_ref().ok_or("no from user")?;
            activate_or_refuse(repos.promo, user, code.to_owned(), lang_code).await?
        },
        None => {
            t!("commands.promo.request", locale = &lang_code).to_string()
        }
    };
    reply_html!(bot, msg, answer);
    Ok(())
}

pub fn promo_inline_filter(InlineQuery { query, .. }: InlineQuery) -> bool {
    PromoCode::new(query).is_ok()
}

#[autometrics]
#[tracing::instrument(skip_all, fields(uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn promo_inline_handler(bot: Bot, query: InlineQuery, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::INLINE_COUNTER.invoked();

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

pub(crate) async fn promo_activation_impl(
    promo_repo: repo::Promo,
    user: &User,
    promo_code: &PromoCode,
    lang_code: LanguageCode,
) -> anyhow::Result<String> {
    let answer = match promo_repo.activate(UserId::from(user), promo_code).await {
        Ok(res) => {
            metrics::CMD_PROMO.finished.inc();
            let plurality = if res.chats_affected.several() {
                "plural"
            } else {
                "singular"
            };
            let bonus = res.bonus_length.value();
            let direction = if bonus.is_negative() { "shrunk" } else { "grown" };
            let chats_in_russian = get_chats_in_russian(res.chats_affected);
            t!("commands.promo.success.template", locale = &lang_code,
                ending = t!(&format!("commands.promo.success.{direction}.{plurality}"), locale = &lang_code,
                    growth = bonus.unsigned_abs(), affected_chats = res.chats_affected,
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

/// A code the type refuses never reaches the database.
async fn activate_or_refuse(
    promo_repo: repo::Promo,
    user: &User,
    code: String,
    lang_code: LanguageCode,
) -> anyhow::Result<String> {
    match PromoCode::new(code) {
        Ok(code) => promo_activation_impl(promo_repo, user, &code, lang_code).await,
        Err(e) => {
            tracing::info!(error = %e, "the promo code doesn't fit the format");
            Ok(t!("commands.promo.errors.invalid_format", locale = &lang_code).to_string())
        }
    }
}

fn get_chats_in_russian(count: AffectedRows) -> String {
    if count.single() {
        "чате"
    } else {
        "чатах"
    }.to_owned()
}


#[cfg(test)]
mod test {
    use teloxide::types::{InlineQuery, InlineQueryId, User, UserId};
    use super::promo_inline_filter;

    fn query(text: &str) -> InlineQuery {
        InlineQuery {
            id: InlineQueryId("1".to_owned()),
            from: User {
                id: UserId(1),
                is_bot: false,
                first_name: "test".to_owned(),
                last_name: None,
                username: None,
                language_code: None,
                is_premium: false,
                added_to_attachment_menu: false,
            },
            query: text.to_owned(),
            offset: String::default(),
            chat_type: None,
            location: None,
        }
    }

    /// The inline button is offered for what the type would accept, and for nothing else.
    #[test]
    fn only_a_well_formed_code_is_offered() {
        for accepted in ["TESTPROMO", "test-11_1", "промо-код", "тест1234"] {
            assert!(promo_inline_filter(query(accepted)), "{accepted} should be offered");
        }
        for refused in ["T34", "PROMO!", "VERYVERYLONGLONGPROMOCODE"] {
            assert!(!promo_inline_filter(query(refused)), "{refused} should not be offered");
        }
    }
}
