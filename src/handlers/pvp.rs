use std::borrow::Cow;
use autometrics::autometrics;
use anyhow::{anyhow, Context};
use domain_types::traits::SaturatingInto;
use crate::errors::flat::flat;
use crate::metrics::RejectionReason;
use crate::domain::primitives::Coefficient;
use futures::join;
use rand::RngExt;
use rust_i18n::t;
use teloxide::macros::BotCommands;
use teloxide::payloads::{AnswerInlineQuerySetters, EditMessageTextInlineSetters};
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, ChosenInlineResult, InlineKeyboardButton, InlineKeyboardMarkup, InlineQuery,
                      InlineQueryResult, InlineQueryResultArticle, InputMessageContent, InputMessageContentText,
                      Message, ParseMode, ReplyMarkup};
use teloxide::types::User as TeloxideUser;
use crate::config::{AppConfig, FeatureToggles, ThrottledBot};
use crate::handlers::{reply_html, send_error_callback_answer, utils, CallbackButton, CallbackResult, HandlerDeps,
    HandlerImplResult, HandlerResult};
use crate::{metrics, reply_html, reply_html_ephemeral, repo};
use crate::config::{BattlesFeatureToggles, MessageGroup};
use crate::domain::objects::{BattleStats, GrowthResult, User, WinRateAware};
use crate::domain::primitives::{Bet, CharCount, LanguageCode, LengthChange, LoanPayout, UserId, Username};
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality, InlineMessageId, TelegramChatId};
use crate::handlers::utils::callbacks;
use crate::handlers::utils::SelfDestructionService;
use crate::handlers::utils::callbacks::{CallbackDataWithPrefix, InvalidCallbackDataBuilder, NewLayoutValue};
use crate::handlers::utils::locks::BattleLocks;
use crate::repo::Repositories;

// let's calculate time offsets from 22.06.2024
const TIMESTAMP_MILLIS_SINCE_2024: i64 = 1719014400000;

#[derive(BotCommands, Clone, Copy)]
#[command(rename_rule = "lowercase")]
pub enum BattleCommands {
    #[command(description = "pvp")]
    Pvp(u32),
    Battle(u32),
    Attack(u32),
    Fight(u32),
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
    fn bet(&self) -> u32 {
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
    bet: Bet,

    // used to prevent repeated clicks on the same button
    timestamp: NewLayoutValue<i64>
}

impl BattleCallbackData {
    fn new(initiator: UserId, bet: Bet) -> Self {
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
        let initiator = callbacks::parse_part(&mut parts, &err, "uid").map(UserId::new)?;
        let bet = callbacks::parse_part(&mut parts, &err, "bet").map(Bet::new)?;
        let timestamp = callbacks::parse_optional_part(&mut parts, &err)?;
        Ok(Self { initiator, bet, timestamp })
    }
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn pvp_cmd_handler(
    bot: ThrottledBot,
    msg: Message,
    cmd: BattleCommands,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, config, self_destruction, lang_resolver } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_PVP_COUNTER.chat.inc();

    let user = msg.from.as_ref().ok_or(anyhow!("no FROM field in the PVP command handler"))?.into();
    let params = BattleParams {
        repos,
        features: config.features.pvp,
        chat_id: msg.chat.id.into(),
        lang_code: lang_code.clone(),
    };
    let bet = Bet::new(cmd.bet());
    let answer = pvp_impl_start(params, user, bet).await?;

    reply_html_ephemeral!(bot, msg, answer.text(), self_destruction, MessageGroup::Application, lang_code,
        reply_markup = answer.keyboard().map(ReplyMarkup::InlineKeyboard));
    Ok(())
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = msg.chat.id.0, uid = ?crate::handlers::msg_user_id(&msg), lang_code = tracing::field::Empty))]
pub async fn pvp_cmd_handler_no_args(bot: ThrottledBot, msg: Message, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::CMD_PVP_COUNTER.chat.inc();
    metrics::CMD_PVP_REJECTED.rejected(RejectionReason::MissingArgument);

    reply_html!(bot, msg, t!("commands.pvp.errors.no_args", locale = &lang_code));
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

#[autometrics]
#[tracing::instrument(skip_all, fields(uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn pvp_inline_handler(bot: ThrottledBot, query: InlineQuery, deps: HandlerDeps) -> HandlerResult {
    let HandlerDeps { lang_resolver, .. } = deps;
    let lang_code = lang_resolver.execute().await;
    metrics::INLINE_COUNTER.invoked();

    let bet = Bet::new(query.query.parse()?);
    let name = utils::get_full_name(&query.from);
    let res = build_inline_keyboard_article_result(query.from.id.into(), &lang_code, &name, bet);

    let mut answer = bot.answer_inline_query(query.id.clone(), [res])
        .is_personal(true);
    if cfg!(debug_assertions) {
        answer.cache_time.replace(1);
    }
    answer.await.context("couldn't answer the inline query")?;
    Ok(())
}

pub(super) fn build_inline_keyboard_article_result(
    uid: UserId,
    lang_code: &LanguageCode,
    name: &Username,
    bet: Bet,
) -> InlineQueryResult {
    tracing::debug!(uid = %uid, bet = %bet, "building an inline PvP offer");

    let texts = OfferTexts::new(lang_code, name, bet);
    let content = InputMessageContent::Text(InputMessageContentText::new(texts.text).parse_mode(ParseMode::Html));
    let btn_data = BattleCallbackData::new(uid, bet).to_data_string();
    InlineQueryResultArticle::new("pvp", texts.title, content)
        .reply_markup(InlineKeyboardMarkup::new([[
            InlineKeyboardButton::callback(texts.button, btn_data)
        ]]))
        .into()
}

#[autometrics]
#[tracing::instrument(skip_all, fields(uid = result.from.id.0, lang_code = tracing::field::Empty))]
pub async fn pvp_inline_chosen_handler(
    bot: ThrottledBot,
    result: ChosenInlineResult,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, self_destruction, lang_resolver, config } = deps;
    metrics::INLINE_COUNTER.finished();

    // Telegram posts the offer out of the article, so this is the first (and only) moment the bot
    // learns which message carries it — and the only thing it knows about the chat is whatever the
    // id encodes. The text isn't handed back, so it is built again for the reading time.
    let Some(inline_message_id) = result.inline_message_id.map(InlineMessageId::new) else {
        return Ok(())
    };
    let AppConfig { features, .. } = config;
    let lang_code = lang_resolver.execute().await;
    let chat = determine_offer_chat(features, &inline_message_id);
    let bet = Bet::new(result.query.parse()?);
    let name = utils::get_full_name(&result.from);
    let texts = OfferTexts::new(&lang_code, &name, bet);

    if can_afford(&repos, features.pvp, &chat, result.from.id.into(), bet).await? {
        schedule_offer(&self_destruction, inline_message_id, chat, &texts).await;
    } else {
        refuse_offer(&bot, &self_destruction, inline_message_id, chat, &texts).await?;
    }
    Ok(())
}

pub fn callback_filter(query: CallbackQuery) -> bool {
    BattleCallbackData::check_prefix(query)
}

#[autometrics]
#[tracing::instrument(skip_all, fields(chat_id = ?crate::handlers::cq_chat_id(&query), uid = query.from.id.0, lang_code = tracing::field::Empty))]
pub async fn pvp_callback_handler(
    bot: ThrottledBot,
    query: CallbackQuery,
    battle_locker: BattleLocks,
    deps: HandlerDeps,
) -> HandlerResult {
    let HandlerDeps { repos, config, self_destruction, lang_resolver } = deps;
    let lang_code = lang_resolver.execute().await;
    let chat_id: ChatIdPartiality = query.message.as_ref()
        .map(|msg| msg.chat().id)
        .map(TelegramChatId::from)
        .or_else(|| config.features.chats_merging
            .then_some(query.inline_message_id.as_ref())
            .flatten()
            .and_then(|msg_id| utils::resolve_inline_message_id(msg_id)
                .inspect_err(|e| tracing::error!(error = flat!(e), "couldn't resolve the inline_message_id"))
                .ok()
            )
            .and_then(|info| info.chat_id)
        )
        .map(ChatIdPartiality::from)
        .unwrap_or(ChatIdPartiality::from(query.chat_instance.clone()));

    let callback_data = BattleCallbackData::parse(&query)?;
    if callback_data.initiator == query.from.id {
        metrics::CMD_PVP_REJECTED.rejected(RejectionReason::SamePerson);
        return send_error_callback_answer(bot, query, "commands.pvp.errors.same_person").await;
    }
    let _battle_guard = match battle_locker.try_lock(&callback_data).await {
        Some(lock) => lock,
        None => {
            metrics::CMD_PVP_REJECTED.rejected(RejectionReason::AlreadyInProgress);
            return send_error_callback_answer(bot, query, "commands.pvp.errors.battle_already_in_progress").await
        },
    };

    let params = BattleParams {
        repos,
        features: config.features.pvp,
        lang_code,
        chat_id: chat_id.clone(),
    };
    let attack_result = pvp_impl_attack(params, callback_data.initiator, query.from.clone().into(), callback_data.bet).await?;
    // A fought battle is a record of itself, not an offer waiting to expire.
    match (query.message.as_ref(), query.inline_message_id.as_ref()) {
        (Some(message), _) => self_destruction.cancel_message(message.chat().id, message.id()).await,
        (None, Some(inline_message_id)) =>
            self_destruction.cancel_inline(InlineMessageId::new(inline_message_id.clone())).await,
        (None, None) => {},
    };
    attack_result.apply(bot, query).await?;

    metrics::CMD_PVP_COUNTER.inline.inc();
    Ok(())
}

pub(crate) struct BattleParams {
    repos: Repositories,
    features: BattlesFeatureToggles,
    chat_id: ChatIdPartiality,
    lang_code: LanguageCode,
}

#[derive(Clone)]
pub(crate) struct UserInfo {
    uid: UserId,
    name: Username,
}

impl From<&TeloxideUser> for UserInfo {
    fn from(value: &TeloxideUser) -> Self {
        Self {
            uid: value.id.into(),
            name: utils::get_full_name(value)
        }
    }
}

impl From<TeloxideUser> for UserInfo {
    fn from(value: TeloxideUser) -> Self {
        (&value).into()
    }
}

impl From<User> for UserInfo {
    fn from(value: User) -> Self {
        Self {
            uid: value.uid,
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

pub(crate) async fn pvp_impl_start(
    p: BattleParams,
    initiator: UserInfo,
    bet: Bet,
) -> anyhow::Result<HandlerImplResult<BattleCallbackData>> {
    let enough = p.repos.dicks.check_dick(&p.chat_id.kind(), initiator.uid, bet).await?;
    tracing::debug!(uid = %initiator.uid, chat_id = %p.chat_id, bet = %bet, enough, "starting a PvP");

    let data = if enough {
        let text = t!("commands.pvp.results.start", locale = &p.lang_code, name = initiator.name.escaped(), bet = bet).into_owned();
        let btn_label = t!("commands.pvp.button", locale = &p.lang_code).into_owned();
        let btn_data = BattleCallbackData::new(initiator.uid, bet);
        HandlerImplResult::WithKeyboard { text, buttons: vec![CallbackButton::new(btn_label, btn_data)] }
    } else {
        metrics::CMD_PVP_REJECTED.rejected(RejectionReason::NotEnoughInitiator);
        HandlerImplResult::OnlyText(t!("commands.pvp.errors.not_enough.initiator", locale = &p.lang_code).into_owned())
    };
    Ok(data)
}

async fn pvp_impl_attack(
    p: BattleParams,
    initiator: UserId,
    acceptor: UserInfo,
    bet: Bet,
) -> anyhow::Result<CallbackResult> {
    let chat_id_kind = p.chat_id.kind();
    let (enough_initiator, enough_acceptor) = join!(
       p.repos.dicks.check_dick(&chat_id_kind, initiator, bet),
       p.repos.dicks.check_dick(&chat_id_kind, acceptor.uid, if p.features.check_acceptor_length { bet } else { Bet::new(0) }),
    );
    let (enough_initiator, enough_acceptor) = (enough_initiator?, enough_acceptor?);

    tracing::debug!(chat_id = %p.chat_id, initiator = %initiator, acceptor = %acceptor.uid,
        enough_initiator, enough_acceptor, bet = %bet, "executing the battle");

    let result = if enough_initiator && enough_acceptor {
        let acceptor_uid = acceptor.clone().into();
        let (winner, loser) = choose_winner(initiator, acceptor_uid);
        let (loser_res, winner_res) = p.repos.dicks.move_length(&p.chat_id, loser, winner, bet).await?;

        let battle_stats = p.repos.pvp_stats.send_battle_result(&p.chat_id.kind(), winner, loser, bet).await
            .inspect_err(|e| tracing::error!(winner = %winner, loser = %loser, error = flat!(e), "couldn't send the battle statistics"))
            .ok()
            .filter(|_| p.features.show_stats)
            .map(|BattleStats { winner: winner_stats, loser: loser_stats }| {
                let mut stats_str = t!("commands.pvp.results.stats.text", locale = &p.lang_code,
                    winner_win_rate = winner_stats.win_rate_percentage(), loser_win_rate = loser_stats.win_rate_percentage(),
                    winner_win_streak = winner_stats.win_streak_current, winner_win_streak_max = winner_stats.win_streak_max,
                ).into_owned();
                if loser_stats.prev_win_streak > 1 {
                    stats_str.push('\n');
                    stats_str.push_str(&t!("commands.pvp.results.stats.lost_win_streak", locale = &p.lang_code,
                        lost_win_streak = loser_stats.prev_win_streak));
                }
                stats_str
            })
            .map(|s| format!("\n\n{s}"))
            .unwrap_or_default();
        
        let (winner_res, withheld_part) = pay_for_loan_if_needed(&p, winner, bet).await
            .inspect_err(|e| tracing::error!(error = flat!(e), "couldn't pay for a loan from a battle award"))
            .ok().flatten()
            .filter(|(_, withheld)| *withheld > 0)
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
            main_part.to_string()
        };
        CallbackResult::EditMessage(format!("{text}{withheld_part}{battle_stats}"), None)
    } else if enough_acceptor {
        metrics::CMD_PVP_REJECTED.rejected(RejectionReason::NotEnoughInitiator);
        let text = t!("commands.pvp.errors.not_enough.initiator", locale = &p.lang_code).into_owned();
        CallbackResult::EditMessage(text, None)
    } else {
        metrics::CMD_PVP_REJECTED.rejected(RejectionReason::NotEnoughAcceptor);
        let text = t!("commands.pvp.errors.not_enough.acceptor", locale = &p.lang_code).into_owned();
        CallbackResult::ShowError(text)
    };
    Ok(result)
}

fn choose_winner<T>(initiator: T, acceptor: T) -> (T, T) {
    if rand::rng().random_bool(0.5) {
        (acceptor, initiator)
    } else {
        (initiator, acceptor)
    }
}

async fn get_user_info(users: &repo::Users, user_uid: UserId, acceptor: &UserInfo) -> anyhow::Result<UserInfo> {
    let user = if user_uid == acceptor.uid {
        acceptor.clone()
    } else {
        users.get_user(user_uid).await?
            .ok_or(anyhow!("pvp participant must present in the database!"))?
            .into()
    };
    Ok(user)
}

async fn pay_for_loan_if_needed(
    p: &BattleParams,
    winner_id: UserId,
    award: Bet,
) -> anyhow::Result<Option<(GrowthResult, LoanPayout)>> {
    let chat_id_kind = p.chat_id.kind();
    let loan = match p.repos.loans.get_active_loan(winner_id, &chat_id_kind).await? {
        Some(loan) => loan,
        None => return Ok(None)
    };
    // the ratio is within [0; 1], so the payout never exceeds the award
    let payout_value: i64 = loan.payout_ratio.scale(award.value().into()).round().saturating_into();
    let payout_value = payout_value.min(loan.debt.saturating_into());
    let payout = LoanPayout::new(payout_value.saturating_into());

    p.repos.loans.pay(winner_id, &chat_id_kind, payout).await?;

    let withheld = LengthChange::signed(-i64::from(payout.value()));
    let growth_res = p.repos.dicks.grow_no_attempts_check(&chat_id_kind, winner_id, withheld).await?;
    Ok(Some((growth_res, payout)))
}

fn new_short_timestamp() -> NewLayoutValue<i64> {
    NewLayoutValue::Some(chrono::Utc::now().timestamp_millis() - TIMESTAMP_MILLIS_SINCE_2024)
}

/// The chat an inline offer was posted in, when its id encodes one.
fn determine_offer_chat(features: FeatureToggles, inline_message_id: &InlineMessageId) -> Option<ChatIdKind> {
    features.chats_merging
        .then(|| utils::try_resolve_chat_id(inline_message_id))
        .flatten()
        .map(ChatIdKind::from)
}

/// Whether the initiator's dick covers the bet. With no chat to look in, nobody can tell, so it does.
async fn can_afford(
    repos: &Repositories,
    features: BattlesFeatureToggles,
    chat: &Option<ChatIdKind>,
    uid: UserId,
    bet: Bet,
) -> anyhow::Result<bool> {
    match chat {
        Some(chat) if features.check_inline_offer => repos.dicks.check_dick(chat, uid, bet).await,
        _ => Ok(true),
    }
}

async fn schedule_offer(
    self_destruction: &SelfDestructionService,
    inline_message_id: InlineMessageId,
    chat: Option<ChatIdKind>,
    texts: &OfferTexts<'_>,
) {
    let char_count = CharCount::of(&texts.text);
    self_destruction.schedule_inline(inline_message_id, chat,
        MessageGroup::Application, char_count, texts.lang_code).await;
}

/// Turns the posted offer into a notice that the initiator is short of the bet, which also takes its
/// button away.
async fn refuse_offer(
    bot: &ThrottledBot,
    self_destruction: &SelfDestructionService,
    inline_message_id: InlineMessageId,
    chat: Option<ChatIdKind>,
    texts: &OfferTexts<'_>,
) -> anyhow::Result<()> {
    metrics::CMD_PVP_REJECTED.rejected(RejectionReason::NotEnoughInitiator);
    let text = texts.not_enough();
    let char_count = CharCount::of(&text);
    bot.edit_message_text_inline(inline_message_id.value(), text)
        .parse_mode(ParseMode::Html)
        .await
        .context("couldn't refuse the inline PvP offer")?;
    self_destruction.schedule_inline(inline_message_id, chat,
        MessageGroup::Notice, char_count, texts.lang_code).await;
    Ok(())
}

/// What an inline offer says: its entry in the list of results, the message posted and its button.
struct OfferTexts<'a> {
    title: Cow<'a, str>,
    text: Cow<'a, str>,
    button: Cow<'a, str>,

    lang_code: &'a LanguageCode,
}

impl<'a> OfferTexts<'a> {
    fn new(lang_code: &'a LanguageCode, name: &Username, bet: Bet) -> Self {
        Self {
            title: t!("inline.results.titles.pvp", locale = lang_code, bet = bet),
            text:  t!("commands.pvp.results.start", locale = lang_code, name = name.escaped(), bet = bet),
            button: t!("commands.pvp.button", locale = lang_code),

            lang_code,
        }
    }

    /// What the offer turns into when the initiator is short of the bet. Only a refusal needs it.
    fn not_enough(&self) -> Cow<'a, str> {
        t!("commands.pvp.errors.not_enough.initiator", locale = self.lang_code)
    }
}
