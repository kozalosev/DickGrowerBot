use anyhow::anyhow;
use futures::join;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::{AnswerCallbackQuerySetters, AnswerInlineQuerySetters};
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, ChatId, ChosenInlineResult, InlineKeyboardButton, InlineKeyboardMarkup, InlineQuery, InlineQueryResultArticle, InputMessageContent, InputMessageContentText, Message, ParseMode, ReplyMarkup, User, UserId};
use crate::handlers::{CallbackResult, ensure_lang_code, HandlerResult, reply_html, utils};
use crate::{impl_username_aware, metrics, repo};
use crate::config::{AppConfig, BattlesFeatureToggles};
use crate::handlers::utils::username::UserNameAware;
use crate::repo::{ChatIdPartiality, GrowthResult, Repositories};

const CALLBACK_PREFIX: &str = "pvp:";

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

    let bet: u32 = query.query.parse()?;
    let lang_code = ensure_lang_code(Some(&query.from));
    let name = utils::get_full_name(&query.from);

    let title = t!("inline.results.titles.pvp", locale = &lang_code, bet = bet);
    let text = t!("commands.pvp.results.start", locale = &lang_code, name = name, bet = bet);
    let content = InputMessageContent::Text(InputMessageContentText::new(text).parse_mode(ParseMode::Html));
    let btn_label = t!("commands.pvp.button", locale = &lang_code);
    let btn_data = format!("{CALLBACK_PREFIX}{}:{bet}", query.from.id);
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
    query.data
        .filter(|d| d.starts_with(CALLBACK_PREFIX))
        .is_some()
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery, repos: Repositories, config: AppConfig) -> HandlerResult {
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

    let params = BattleParams {
        repos,
        features: config.features.pvp,
        lang_code: ensure_lang_code(Some(&query.from)),
        chat_id: chat_id.clone(),
    };
    let (initiator, bet) = parse_data(&query.data)?;
    if initiator == query.from.id {
        bot.answer_callback_query(query.id)
            .show_alert(true)
            .text(t!("commands.pvp.errors.same_person", locale = &params.lang_code))
            .await?;
        return Ok(())
    }

    let attack_result = pvp_impl_attack(params, initiator, query.from.clone().into(), bet).await?;
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
    name: String,
}
impl_username_aware!(UserInfo);

impl From<&User> for UserInfo {
    fn from(value: &User) -> Self {
        Self {
            uid: value.id,
            name: utils::get_full_name(value)
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
        let text = t!("commands.pvp.results.start", locale = &p.lang_code, name = initiator.username_escaped(), bet = bet);
        let btn_label = t!("commands.pvp.button", locale = &p.lang_code);
        let btn_data = format!("{CALLBACK_PREFIX}{}:{bet}", initiator.uid);
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
            winner_name = winner_info.username_escaped(), winner_length = winner_res.new_length, loser_length = loser_res.new_length, bet = bet);
        let text = if let (Some(winner_pos), Some(loser_pos)) = (winner_res.pos_in_top, loser_res.pos_in_top) {
            let winner_pos = t!("commands.pvp.results.position.winner", locale = &p.lang_code, name = winner_info.username_escaped(), pos = winner_pos);
            let loser_pos = t!("commands.pvp.results.position.loser", locale = &p.lang_code, name = loser_info.username_escaped(), pos = loser_pos);
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

fn parse_data(maybe_data: &Option<String>) -> anyhow::Result<(UserId, u16)> {
    let parts = maybe_data.as_ref()
        .and_then(|data| data.strip_prefix(CALLBACK_PREFIX).map(|s| s.to_owned()))
        .map(|data| data.split(':').map(|s| s.to_owned()).collect::<Vec<String>>())
        .ok_or(anyhow!("callback data must be present!"))?;
    if parts.len() == 2 {
        let uid: u64 = parts[0].parse()?;
        let bet: u16 = parts[1].parse()?;
        Ok((UserId(uid), bet))
    } else {
        Err(anyhow!("invalid number of arguments ({}) in the callback data: {:?}", parts.len(), parts))
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
