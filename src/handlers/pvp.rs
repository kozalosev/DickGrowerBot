use anyhow::anyhow;
use futures::join;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::AnswerInlineQuerySetters;
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, ChatId, ChosenInlineResult, InlineKeyboardButton, InlineKeyboardMarkup, InlineQuery, InlineQueryResultArticle, InputMessageContent, InputMessageContentText, Message, ParseMode, ReplyMarkup, User, UserId};
use crate::handlers::{CallbackResult, ensure_lang_code, HandlerResult, reply_html, send_error_callback_answer, utils};
use crate::{metrics, repo};
use crate::config::{AppConfig, BattlesFeatureToggles};
use crate::domain::Username;
use crate::handlers::utils::callbacks;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackDataBuilder, NewLayoutValue};
use crate::handlers::utils::locks::LockCallbackServiceFacade;
use crate::repo::{ChatIdPartiality, GrowthResult, Repositories};

// let's calculate time offsets from 22.06.2024
const TIMESTAMP_MILLIS_SINCE_2024: i64 = 1719014400000;

#[derive(BotCommands, Clone, Copy)]
#[command(rename_rule = "lowercase")]
pub enum BattleCommands {
    #[command(description = "pvp")]
    Pvp(u16),
    Battle(u16),
    Attack(u16),
    Fight(u16),
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum BattleCommandsNoArgs {
    Pvp,
    Battle,
    Attack,
    Fight,
}

impl BattleCommands {
    fn bet(&self) -> u16 {
        match *self {
            Self::Battle(bet) => bet,
            Self::Pvp(bet) => bet,
            Self::Attack(bet) => bet,
            Self::Fight(bet) => bet,
        }
    }
}

#[derive(derive_more::Display)]
#[display("{initiator}:{bet}:{timestamp}")]
pub(crate) struct BattleCallbackData {
    initiator: UserId,
    bet: u16,

    // used to prevent repeated clicks on the same button
    timestamp: NewLayoutValue<i64>
}

impl BattleCallbackData {
    fn new(initiator: UserId, bet: u16) -> Self {
        Self {
            initiator, bet,
            timestamp: new_short_timestamp()
        }
    }
}

impl CallbackDataWithPrefix for BattleCallbackData {
    fn prefix() -> &'static str {
        "pvp"
    }
}

impl TryFrom<String> for BattleCallbackData {
    type Error = callbacks::InvalidCallbackData;

    fn try_from(data: String) -> Result<Self, Self::Error> {
        let err = InvalidCallbackDataBuilder(&data);
        let mut parts = data.split(':');
        let initiator = callbacks::parse_part(&mut parts, &err, "uid").map(UserId)?;
        let bet: u16 = callbacks::parse_part(&mut parts, &err, "bet")?;
        let timestamp = callbacks::parse_optional_part(&mut parts, &err)?;
        Ok(Self { initiator, bet, timestamp })
    }
}

pub async fn cmd_handler(bot: Bot, msg: Message, cmd: BattleCommands,
                         repos: Repositories, config: AppConfig) -> HandlerResult {
    metrics::CMD_PVP_COUNTER.chat.inc();

    let user = msg.from().ok_or(anyhow!("no FROM field in the PVP command handler"))?.into();
    let lang_code = ensure_lang_code(msg.from());
    let params = BattleParams {
        repos,
        features: config.features.pvp,
        chat_id: msg.chat.id.into(),
        lang_code,
    };
    let (text, keyboard) = pvp_impl_start(params, user, cmd.bet()).await?;

    let mut answer = reply_html(bot, msg, text);
    answer.reply_markup = keyboard.map(ReplyMarkup::InlineKeyboard);
    answer.await?;
    Ok(())
}

pub async fn cmd_handler_no_args(bot: Bot, msg: Message) -> HandlerResult {
    metrics::CMD_PVP_COUNTER.chat.inc();

    let lang_code = ensure_lang_code(msg.from());
    reply_html(bot, msg, t!("commands.pvp.errors.no_args", locale = &lang_code)).await?;
    Ok(())
}

pub fn inline_filter(query: InlineQuery) -> bool {
    let maybe_bet: Result<u32, _> = query.query.parse();
    maybe_bet.is_ok()
}

pub fn chosen_inline_result_filter(result: ChosenInlineResult) -> bool {
    let maybe_bet: Result<u32, _> = result.query.parse();
    maybe_bet.is_ok()
}

pub async fn inline_handler(bot: Bot, query: InlineQuery) -> HandlerResult {
    metrics::INLINE_COUNTER.invoked();

    let bet: u16 = query.query.parse()?;
    let lang_code = ensure_lang_code(Some(&query.from));
    let name = utils::get_full_name(&query.from);

    let title = t!("inline.results.titles.pvp", locale = &lang_code, bet = bet);
    let text = t!("commands.pvp.results.start", locale = &lang_code, name = name, bet = bet);
    let content = InputMessageContent::Text(InputMessageContentText::new(text).parse_mode(ParseMode::Html));
    let btn_label = t!("commands.pvp.button", locale = &lang_code);
    let btn_data = BattleCallbackData::new(query.from.id, bet).to_data_string();
    let res = InlineQueryResultArticle::new("pvp", title, content)
        .reply_markup(InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback(btn_label, btn_data)
        ]]))
        .into();

    let mut answer = bot.answer_inline_query(query.id, vec![res])
        .is_personal(true);
    if cfg!(debug_assertions) {
        answer.cache_time.replace(1);
    }
    answer.await?;
    Ok(())
}

pub async fn inline_chosen_handler() -> HandlerResult {
    metrics::INLINE_COUNTER.finished();
    Ok(())
}

pub fn callback_filter(query: CallbackQuery) -> bool {
    BattleCallbackData::check_prefix(query)
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery, repos: Repositories, config: AppConfig,
                              mut battle_locker: LockCallbackServiceFacade) -> HandlerResult {
    let chat_id: ChatIdPartiality = query.message.as_ref()
        .map(|msg| msg.chat.id)
        .or_else(|| config.features.chats_merging
            .then_some(query.inline_message_id.as_ref())
            .flatten()
            .and_then(|msg_id| utils::resolve_inline_message_id(msg_id)
                .inspect_err(|e| log::error!("couldn't resolve inline_message_id: {e}"))
                .ok()
            )
            .map(|info| ChatId(info.chat_id))
        )
        .map(ChatIdPartiality::from)
        .unwrap_or(ChatIdPartiality::from(query.chat_instance.clone()));

    let callback_data = BattleCallbackData::parse(&query)?;
    if callback_data.initiator == query.from.id {
        return send_error_callback_answer(bot, query, "commands.pvp.errors.same_person").await;
    }
    let _battle_guard = match battle_locker.try_lock(&callback_data) {
        Some(lock) => lock,
        None => return send_error_callback_answer(bot, query, "commands.pvp.errors.battle_already_in_progress").await
    };

    let params = BattleParams {
        repos,
        features: config.features.pvp,
        lang_code: ensure_lang_code(Some(&query.from)),
        chat_id: chat_id.clone(),
    };
    let attack_result = pvp_impl_attack(params, callback_data.initiator, query.from.clone().into(), callback_data.bet).await?;
    attack_result.apply(bot, query).await?;

    metrics::CMD_PVP_COUNTER.inline.inc();
    Ok(())
}

pub(crate) struct BattleParams {
    repos: Repositories,
    features: BattlesFeatureToggles,
    chat_id: ChatIdPartiality,
    lang_code: String,
}

#[derive(Clone)]
pub(crate) struct UserInfo {
    uid: UserId,
    name: Username,
}

impl From<&User> for UserInfo {
    fn from(value: &User) -> Self {
        Self {
            uid: value.id,
            name: Username::new(utils::get_full_name(value))
        }
    }
}

impl From<User> for UserInfo {
    fn from(value: User) -> Self {
        (&value).into()
    }
}

impl From<repo::User> for UserInfo {
    fn from(value: repo::User) -> Self {
        Self {
            uid: UserId(value.uid as u64),
            name: value.name
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<UserId> for UserInfo {
    fn into(self) -> UserId {
        self.uid
    }
}

pub(crate) async fn pvp_impl_start(p: BattleParams, initiator: UserInfo, bet: u16) -> anyhow::Result<(String, Option<InlineKeyboardMarkup>)> {
    let enough = p.repos.dicks.check_dick(&p.chat_id.kind(), initiator.uid, bet).await?;
    let data = if enough {
        let text = t!("commands.pvp.results.start", locale = &p.lang_code, name = initiator.name.escaped(), bet = bet);
        let btn_label = t!("commands.pvp.button", locale = &p.lang_code);
        let btn_data = BattleCallbackData::new(initiator.uid, bet).to_data_string();
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback(btn_label, btn_data)
        ]]);
        (text, Some(keyboard))
    } else {
        (t!("commands.pvp.errors.not_enough.initiator", locale = &p.lang_code), None)
    };
    Ok(data)
}

async fn pvp_impl_attack(p: BattleParams, initiator: UserId, acceptor: UserInfo, bet: u16) -> anyhow::Result<CallbackResult> {
    let chat_id_kind = p.chat_id.kind();
    let (enough_initiator, enough_acceptor) = join!(
       p.repos.dicks.check_dick(&chat_id_kind, initiator, bet),
       p.repos.dicks.check_dick(&chat_id_kind, acceptor.uid, if p.features.check_acceptor_length { bet } else { 0 }),
    );
    let (enough_initiator, enough_acceptor) = (enough_initiator?, enough_acceptor?);

    let result = if enough_initiator && enough_acceptor {
        let acceptor_uid = acceptor.clone().into();
        let (winner, loser) = choose_winner(initiator, acceptor_uid);
        let (loser_res, winner_res) = p.repos.dicks.move_length(&p.chat_id, loser, winner, bet).await?;
        
        let (winner_res, withheld_part) = pay_for_loan_if_needed(&p, winner, bet).await
            .inspect_err(|e| log::error!("couldn't pay for a loan from a battle award: {e}"))
            .ok().flatten()
            .map(|(res, withheld)| {
                let withheld_part = format!("\n\n{}", t!("commands.pvp.results.withheld", locale = &p.lang_code, payout = withheld));
                (res, withheld_part)
            })
            .unwrap_or((winner_res, String::default()));

        let winner_info = get_user_info(&p.repos.users, winner, &acceptor).await?;
        let loser_info = get_user_info(&p.repos.users, loser, &acceptor).await?;
        let main_part = t!("commands.pvp.results.finish", locale = &p.lang_code,
            winner_name = winner_info.name.escaped(), winner_length = winner_res.new_length, loser_length = loser_res.new_length, bet = bet);
        let text = if let (Some(winner_pos), Some(loser_pos)) = (winner_res.pos_in_top, loser_res.pos_in_top) {
            let winner_pos = t!("commands.pvp.results.position.winner", locale = &p.lang_code, name = winner_info.name.escaped(), pos = winner_pos);
            let loser_pos = t!("commands.pvp.results.position.loser", locale = &p.lang_code, name = loser_info.name.escaped(), pos = loser_pos);
            format!("{main_part}\n\n{winner_pos}\n{loser_pos}")
        } else {
            main_part
        };
        CallbackResult::EditMessage(format!("{text}{withheld_part}"), None)
    } else if enough_acceptor {
        let text = t!("commands.pvp.errors.not_enough.initiator", locale = &p.lang_code);
        CallbackResult::EditMessage(text, None)
    } else {
        let text = t!("commands.pvp.errors.not_enough.acceptor", locale = &p.lang_code);
        CallbackResult::ShowError(text)
    };
    Ok(result)
}

fn choose_winner<T>(initiator: T, acceptor: T) -> (T, T) {
    if OsRng.gen_bool(0.5) {
        (acceptor, initiator)
    } else {
        (initiator, acceptor)
    }
}

async fn get_user_info(users: &repo::Users, user_uid: UserId, acceptor: &UserInfo) -> anyhow::Result<UserInfo> {
    let user = if user_uid == acceptor.uid {
        acceptor.clone()
    } else {
        users.get(user_uid).await?
            .ok_or(anyhow!("pvp participant must present in the database!"))?
            .into()
    };
    Ok(user)
}

async fn pay_for_loan_if_needed(p: &BattleParams, winner_id: UserId, award: u16) -> anyhow::Result<Option<(GrowthResult, u16)>> {
    let chat_id_kind = p.chat_id.kind();
    let loan = match p.repos.loans.get_active_loan(winner_id, &chat_id_kind).await? {
        Some(loan) => loan,
        None => return Ok(None)
    };
    let payout = (loan.payout_ratio * award as f32).round() as u16;
    let payout = payout.min(loan.debt);
    
    p.repos.loans.pay(winner_id, &chat_id_kind, payout).await?;
    
    let withheld = -(payout as i32);
    let growth_res = p.repos.dicks.grow_no_attempts_check(&chat_id_kind, winner_id, withheld).await?;
    Ok(Some((growth_res, payout)))
}

pub fn new_short_timestamp() -> NewLayoutValue<i64> {
    NewLayoutValue::Some(chrono::Utc::now().timestamp_millis() - TIMESTAMP_MILLIS_SINCE_2024)
}
