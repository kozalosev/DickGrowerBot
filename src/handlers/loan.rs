use std::str::{FromStr, Split};
use anyhow::anyhow;
use derive_more::Display;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::{CallbackQuery, Message, UserId};
use teloxide::requests::Requester;
use teloxide::types::ReplyMarkup;
use callbacks::{EditMessageReqParamsKind, InvalidCallbackData};

use crate::{check_invoked_by_owner_and_get_answer_params, config, metrics, repo};
use crate::handlers::{CallbackButton, ensure_lang_code, FromRefs, HandlerImplResult, HandlerResult, reply_html, try_resolve_chat_id};
use crate::handlers::perks::LoanPayoutPerk;
use crate::handlers::utils::{callbacks, Incrementor};
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackDataBuilder};
use crate::repo::ChatIdPartiality;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum LoanCommands {
    #[command(description = "loan")]
    Loan,
    Borrow,
}

pub async fn cmd_handler(bot: Bot, msg: Message, repos: repo::Repositories, incr: Incrementor) -> HandlerResult {
    metrics::CMD_LOAN_COUNTER.chat.inc();

    let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);

    let result = loan_impl(&repos, from_refs, incr).await?;
    let markup = result.keyboard().map(ReplyMarkup::InlineKeyboard);

    let mut request = reply_html(bot, msg, result.text());
    request.reply_markup = markup;
    request.await?;

    Ok(())
}

pub(crate) async fn loan_impl(repos: &repo::Repositories, from_refs: FromRefs<'_>, incr: Incrementor) -> anyhow::Result<HandlerImplResult<LoanCallbackData>> {
    let (from, chat_id_part) = (from_refs.0, from_refs.1);
    let chat_id_kind = chat_id_part.kind();
    let lang_code = ensure_lang_code(Some(from));
    
    let active_loan = repos.loans.get_active_loan(from.id, &chat_id_kind).await?;
    if active_loan > 0 {
        let left_to_pay = t!("commands.loan.debt", locale = &lang_code, debt = active_loan);
        return Ok(HandlerImplResult::OnlyText(left_to_pay))
    }

    let payout_percentage = if let Some(payout_ratio) = incr.find_perk_config::<LoanPayoutPerk>() {
        payout_ratio * 100.0
    } else {
        let err_text = t!("errors.feature_disabled", locale = &lang_code);
        return Ok(HandlerImplResult::OnlyText(err_text))
    };

    let length = repos.dicks.fetch_length(from.id, &chat_id_kind).await?;
    let res = if length < 0 {
        let debt = length.unsigned_abs() as u16;

        let btn_agree = CallbackButton::new(
            t!("commands.loan.confirmation.buttons.agree", locale = &lang_code),
            LoanCallbackData {
                uid: from.id,
                action: LoanCallbackAction::Confirmed { value: debt }
            }
        );
        let btn_disagree = CallbackButton::new(
            t!("commands.loan.confirmation.buttons.disagree", locale = &lang_code),
            LoanCallbackData {
                uid: from.id,
                action: LoanCallbackAction::Refused
            }
        );
        HandlerImplResult::WithKeyboard {
            text: t!("commands.loan.confirmation.text", locale = &lang_code, debt = debt, payout_percentage = payout_percentage),
            buttons: vec![btn_agree, btn_disagree]
        }
    } else {
        let err_text = t!("commands.loan.errors.positive_length", locale = &lang_code);
        HandlerImplResult::OnlyText(err_text)
    };
    Ok(res)
}

#[inline]
pub fn callback_filter(query: CallbackQuery) -> bool {
    LoanCallbackData::check_prefix(query)
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery,
                              repos: repo::Repositories, config: config::AppConfig) -> HandlerResult {
    let data = LoanCallbackData::parse(&query)?;
    let (answer, lang_code) = check_invoked_by_owner_and_get_answer_params!(bot, query, data.uid);
    let edit_msg_params = callbacks::get_params_for_message_edit(&query)?;

    match data.action {
        LoanCallbackAction::Confirmed { value } => {
            let updated_text = t!("commands.loan.callback.success", locale = &lang_code);
            match edit_msg_params {
                EditMessageReqParamsKind::Chat(chat_id, message_id) => {
                    borrow(repos, chat_id.into(), data.uid, value).await?;
                    bot.edit_message_text(chat_id, message_id, updated_text).await?;
                }
                EditMessageReqParamsKind::Inline { chat_instance, inline_message_id } => {
                    let maybe_chat_id = try_resolve_chat_id(&inline_message_id)
                        // normally, it should be always enabled but let's keep it here for now, just in case
                        .filter(|_| config.features.chats_merging);
                    let chat_id: ChatIdPartiality = if let Some(chat_id) = maybe_chat_id {
                        repos.chats.get_chat(chat_id.into())
                            .await?
                            .and_then(|c| c.try_into().ok())
                            .unwrap_or_else(|| chat_instance.into())
                    } else {
                        chat_instance.into()
                    };

                    borrow(repos, chat_id, data.uid, value).await?;
                    bot.edit_message_text_inline(inline_message_id, updated_text).await?;
                }
            }
        }
        LoanCallbackAction::Refused => {
            let updated_text = t!("commands.loan.callback.refused", locale = &lang_code);
            match edit_msg_params {
                EditMessageReqParamsKind::Chat(chat_id, message_id) => {
                    let unable_to_delete_message = bot.delete_message(chat_id, message_id).await
                        .inspect_err(|e| log::error!("Unable to delete a loan request message: {e}"))
                        .is_err();
                    if unable_to_delete_message {
                        bot.edit_message_text(chat_id, message_id, updated_text).await?;
                    }
                }
                EditMessageReqParamsKind::Inline { inline_message_id, .. } => {
                    bot.edit_message_text_inline(inline_message_id, updated_text).await?;
                }
            }
        }
    }

    answer.await?;
    Ok(())
}

async fn borrow(repos: repo::Repositories, chat_id: ChatIdPartiality, user_id: UserId, debt: u16) -> HandlerResult {
    let loan_tx = repos.loans.borrow(user_id, &chat_id.kind(), debt).await?;
    repos.dicks.create_or_grow(user_id, &chat_id, debt.into()).await?;
    loan_tx.commit().await?;
    Ok(())
}

#[derive(Display)]
#[display("{uid}:{action}")]
pub(crate) struct LoanCallbackData {
    uid: UserId,
    action: LoanCallbackAction
}

#[derive(Display)]
#[cfg_attr(test, derive(PartialEq, Eq, Debug))]
pub(crate) enum LoanCallbackAction {
    #[display("confirmed:{value}")]
    Confirmed { value: u16 },
    #[display("refused")]
    Refused
}

impl CallbackDataWithPrefix for LoanCallbackData {
    fn prefix() -> &'static str {
        "loan"
    }
}

impl TryFrom<String> for LoanCallbackData {
    type Error = InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        let mut parts = data.as_str().split(':');
        let uid = Self::parse_part(&mut parts, &err, "uid").map(UserId)?;
        let action = parts.next()
            .ok_or_else(|| err.missing_part("action"))?;
        let action = match action {
            "confirmed" => {
                let value = Self::parse_part(&mut parts, &err, "value")?;
                LoanCallbackAction::Confirmed { value }
            }
            "refused" => LoanCallbackAction::Refused,
            _ => return Err(err.split_err())
        };
        Ok(Self { uid, action })
    }
}

impl LoanCallbackData {
    fn parse_part<VT, PDT>(parts: &mut Split<char>, err_builder: &InvalidCallbackDataBuilder<VT>, part_name: &str) -> Result<PDT, InvalidCallbackData>
    where
        VT: ToString,
        PDT: FromStr,
        <PDT as FromStr>::Err: std::error::Error + Send + Sync + 'static
    {
        parts.next()
            .ok_or_else(|| err_builder.missing_part(part_name))
            .and_then(|uid| uid.parse().map_err(|e| err_builder.parsing_err(e)))
    }
}


#[cfg(test)]
mod test {
    use teloxide::types::{CallbackQuery, User, UserId};
    use crate::handlers::loan::{LoanCallbackAction, LoanCallbackData};
    use crate::handlers::utils::callbacks::CallbackDataWithPrefix;

    #[test]
    fn test_parse() {
        let (uid, value) = get_test_params();
        let [cd_confirmed, cd_refused] = get_strings(uid, value)
            .map(build_callback_query);
        {
            let lcd_confirmed = LoanCallbackData::parse(&cd_confirmed)
                .expect("callback data for 'confirmed' must be parsed successfully");
            assert_eq!(lcd_confirmed.uid, uid);
            assert_eq!(lcd_confirmed.action, LoanCallbackAction::Confirmed { value });
        }{
            let lcd_refused = LoanCallbackData::parse(&cd_refused)
                .expect("callback data for 'refused' must be parsed successfully");
            assert_eq!(lcd_refused.uid, uid);
            assert_eq!(lcd_refused.action, LoanCallbackAction::Refused)
        }
    }

    #[test]
    fn test_serialize() {
        let (uid, value) = get_test_params();
        let lcd_confirmed = LoanCallbackData {
            uid,
            action: LoanCallbackAction::Confirmed { value }
        };
        let lcd_refused = LoanCallbackData {
            uid,
            action: LoanCallbackAction::Refused
        };

        let [expected_confirmed, expected_refused] = get_strings(uid, value);

        assert_eq!(lcd_confirmed.to_data_string(), expected_confirmed);
        assert_eq!(lcd_refused.to_data_string(), expected_refused);
    }

    fn get_test_params() -> (UserId, u16) {
        (UserId(123456), 10)
    }

    fn get_strings(uid: UserId, value: u16) -> [String; 2] {[
        format!("loan:{uid}:confirmed:{value}"),
        format!("loan:{uid}:refused"),
    ]}

    fn build_callback_query(data: String) -> CallbackQuery {
        CallbackQuery {
            id: "".to_string(),
            from: User {
                id: UserId(0),
                is_bot: false,
                first_name: "".to_string(),
                last_name: None,
                username: None,
                language_code: None,
                is_premium: false,
                added_to_attachment_menu: false,
            },
            message: None,
            inline_message_id: None,
            chat_instance: "".to_string(),
            data: Some(data),   // here we insert a value
            game_short_name: None,
        }
    }
}
