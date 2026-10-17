use autometrics::autometrics;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::prelude::{CallbackQuery, Dialogue, InlineQuery, Requester};
use teloxide::types::{InlineQueryResultsButton, InlineQueryResultsButtonKind, Message, ReplyMarkup, User};
use crate::dialogue::CachedDialogueStorage;
use crate::handlers::{CallbackButton, HandlerDeps, HandlerImplResult, HandlerResult, reply_html};
use crate::handlers::utils::callbacks;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackData, InvalidCallbackDataBuilder};
use crate::{metrics, reply_html, repo};
use crate::cache::Cache;
use crate::domain::primitives::{AffectedRows, LanguageCode, PromoBonus, PromoCode, UserId};
use crate::repo::ActivationError;

pub(crate) const PROMO_START_PARAM_PREFIX: &str = "promo-";

/// A bonus below this asks the user to confirm the activation first.
const CONFIRMATION_THRESHOLD: PromoBonus = PromoBonus::new(-100);

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum PromoCommands {
    #[command(description = "promo")]
    Promo(String),
}

#[derive(Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PromoCommandState {
    #[default]
    Start,
    Requested,
}

pub type PromoCodeDialogue = Dialogue<PromoCommandState, CachedDialogueStorage<PromoCommandState>>;

/// What became of one activation attempt. The refusals are named as the translations of their
/// answers are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum_macros::Display, strum_macros::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum ActivationOutcome {
    Success,
    /// Asked to confirm a code that shrinks.
    Confirmation,
    InvalidFormat,
    NotFound,
    NotStarted,
    Expired,
    Exhausted,
    NoDicks,
    AlreadyActivated,
}

impl TryFrom<ActivationError> for ActivationOutcome {
    type Error = anyhow::Error;

    fn try_from(e: ActivationError) -> Result<Self, Self::Error> {
        let outcome = match e {
            ActivationError::NotFound => Self::NotFound,
            ActivationError::NotStarted => Self::NotStarted,
            ActivationError::Expired => Self::Expired,
            ActivationError::Exhausted => Self::Exhausted,
            ActivationError::NoDicks => Self::NoDicks,
            ActivationError::AlreadyActivated => Self::AlreadyActivated,
            ActivationError::Other(e) => return Err(e),
        };
        Ok(outcome)
    }
}

struct PromoCodeActivationResult {
    answer: String,
    outcome: ActivationOutcome,
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn promo_cmd_handler(
    bot: Bot,
    msg: Message,
    cmd: PromoCommands,
    dialogue: PromoCodeDialogue,
    cache: Cache,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_PROMO.invoked_by_command.inc();
    let user = msg.from.as_ref().ok_or("no from user")?;
    let answer = match cmd {
        PromoCommands::Promo(code) if code.is_empty() => {
            dialogue.update(PromoCommandState::Requested).await?;

            HandlerImplResult::OnlyText(t!("commands.promo.request", locale = &lang_code).to_string())
        }
        PromoCommands::Promo(code) => {
            dialogue.exit().await?;

            activate_or_refuse(repos.promo, &cache, user, code, lang_code).await?
        },
    };
    reply_html!(bot, msg, answer.text(), reply_markup = answer.keyboard().map(ReplyMarkup::InlineKeyboard));
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn promo_requested_handler(
    bot: Bot,
    msg: Message,
    dialogue: PromoCodeDialogue,
    cache: Cache,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let answer = match msg.text() {
        Some(code) => {
            dialogue.exit().await?;

            let user = msg.from.as_ref().ok_or("no from user")?;
            activate_or_refuse(repos.promo, &cache, user, code.to_owned(), lang_code).await?
        },
        None => {
            HandlerImplResult::OnlyText(t!("commands.promo.request", locale = &lang_code).to_string())
        }
    };
    reply_html!(bot, msg, answer.text(), reply_markup = answer.keyboard().map(ReplyMarkup::InlineKeyboard));
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

pub fn promo_callback_filter(query: CallbackQuery) -> bool {
    PromoCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn promo_callback_handler(bot: Bot, query: CallbackQuery, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { repos, lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    let text = match PromoCallbackData::parse(&query)? {
        PromoCallbackData::Confirm(code) => activate(repos.promo, &query.from, &code, &lang_code).await?.answer,
        PromoCallbackData::Cancel => t!("commands.promo.confirmation.cancelled", locale = &lang_code).to_string(),
    };
    let params = callbacks::get_params_for_message_edit(&query)?;
    bot.answer_callback_query(query.id).await?;
    callbacks::edit_message_text_with_keyboard(&bot, params, text, None).await?;
    Ok(())
}

/// Asks for a confirmation first when the code would take a lot away.
pub(crate) async fn promo_activation_impl(
    promo_repo: repo::Promo,
    cache: &Cache,
    user: &User,
    promo_code: &PromoCode,
    lang_code: LanguageCode,
) -> anyhow::Result<HandlerImplResult<PromoCallbackData>> {
    let preview = promo_repo.preview(UserId::from(user), promo_code)
        .await?
        .filter(|p| p.bonus_length < CONFIRMATION_THRESHOLD && !p.chats.zero());
    let (answer, outcome) = match preview {
        Some(preview) => (HandlerImplResult::WithKeyboard {
            text: t!("commands.promo.confirmation.text", locale = &lang_code,
                length = preview.bonus_length.value().unsigned_abs(), chats = preview.chats).to_string(),
            buttons: confirmation_buttons(promo_code, &lang_code),
        }, ActivationOutcome::Confirmation),
        None => {
            let PromoCodeActivationResult { answer, outcome } = activate(promo_repo, user, promo_code, &lang_code).await?;
            (HandlerImplResult::OnlyText(answer), outcome)
        },
    };
    record_attempt(cache, UserId::from(user), outcome).await;
    Ok(answer)
}

async fn activate(
    promo_repo: repo::Promo,
    user: &User,
    promo_code: &PromoCode,
    lang_code: &LanguageCode,
) -> anyhow::Result<PromoCodeActivationResult> {
    let (text, outcome) = match promo_repo.activate(UserId::from(user), promo_code).await {
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
            let text = t!("commands.promo.success.template", locale = &lang_code,
                ending = t!(&format!("commands.promo.success.{direction}.{plurality}"), locale = &lang_code,
                    growth = bonus.unsigned_abs(), affected_chats = res.chats_affected,
                    word_chats = chats_in_russian))
                .to_string();
            (text, ActivationOutcome::Success)
        },
        Err(e) => {
            let outcome = ActivationOutcome::try_from(e)?;
            let t_key = format!("commands.promo.errors.{outcome}");
            (t!(&t_key, locale = &lang_code).to_string(), outcome)
        }
    };
    Ok(PromoCodeActivationResult { answer: text, outcome })
}

/// A code the type refuses never reaches the database.
async fn activate_or_refuse(
    promo_repo: repo::Promo,
    cache: &Cache,
    user: &User,
    code: String,
    lang_code: LanguageCode,
) -> anyhow::Result<HandlerImplResult<PromoCallbackData>> {
    match PromoCode::new(code) {
        Ok(code) => promo_activation_impl(promo_repo, cache, user, &code, lang_code).await,
        Err(e) => {
            tracing::info!(error = %e, "the promo code doesn't fit the format");
            record_attempt(cache, UserId::from(user), ActivationOutcome::InvalidFormat).await;
            let text = t!("commands.promo.errors.invalid_format", locale = &lang_code).to_string();
            Ok(HandlerImplResult::OnlyText(text))
        }
    }
}

async fn record_attempt(cache: &Cache, uid: UserId, outcome: ActivationOutcome) {
    metrics::PROMO_ATTEMPTS.inc(&[outcome.into()]);
    metrics::PROMO_ATTEMPT_USERS.hit(cache, uid).await;
}

fn confirmation_buttons(code: &PromoCode, lang_code: &LanguageCode) -> Vec<CallbackButton<PromoCallbackData>> {
    vec![
        CallbackButton::new(
            t!("commands.promo.confirmation.buttons.confirm", locale = lang_code).to_string(),
            PromoCallbackData::Confirm(code.clone())),
        CallbackButton::new(
            t!("commands.promo.confirmation.buttons.cancel", locale = lang_code).to_string(),
            PromoCallbackData::Cancel),
    ]
}

/// `promo:confirm:<code>` activates the code for whoever presses the button; `promo:cancel` doesn't.
#[derive(Clone, PartialEq, Eq, Debug, derive_more::Display)]
pub(crate) enum PromoCallbackData {
    #[display("confirm:{_0}")]
    Confirm(PromoCode),
    #[display("cancel")]
    Cancel,
}

impl CallbackDataWithPrefix for PromoCallbackData {
    fn prefix() -> &'static str {
        "promo"
    }
}

impl TryFrom<String> for PromoCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        match data.split_once(':') {
            Some(("confirm", code)) => PromoCode::new(code.to_owned())
                .map(Self::Confirm)
                .map_err(|e| err.parsing_err(e)),
            None if data == "cancel" => Ok(Self::Cancel),
            _ => Err(err.split_err()),
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
    use domain_types::literal;
    use crate::domain::primitives::PromoCode;
    use crate::handlers::utils::callbacks::{build_callback_query, CallbackDataWithPrefix};
    use super::{promo_inline_filter, PromoCallbackData};

    /// Every answer survives the button, and the longest code still fits Telegram's 64 bytes.
    #[test]
    fn callback_data_round_trip() {
        let longest = literal!(PromoCode = "промокодпромокод");
        for data in [PromoCallbackData::Confirm(longest), PromoCallbackData::Cancel] {
            let wire = data.to_data_string();
            assert!(wire.len() <= 64, "{wire} is too long for a button");
            let parsed = PromoCallbackData::parse(&build_callback_query(wire))
                .expect("couldn't parse the callback data");
            assert_eq!(parsed, data);
        }
    }

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
        for accepted in ["TESTPROMO", "test-11_1", "промо-код", "тест1234", "T34"] {
            assert!(promo_inline_filter(query(accepted)), "{accepted} should be offered");
        }
        for refused in ["T3", "PROMO!", "VERYVERYLONGLONGPROMOCODE"] {
            assert!(!promo_inline_filter(query(refused)), "{refused} should not be offered");
        }
    }
}
