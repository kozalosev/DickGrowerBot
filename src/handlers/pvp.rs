use std::future::IntoFuture;
use anyhow::anyhow;
use futures::future::join;
use futures::TryFutureExt;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::payloads::{AnswerCallbackQuerySetters, AnswerInlineQuerySetters};
use teloxide::requests::Requester;
use teloxide::types::{CallbackQuery, ChatId, ChosenInlineResult, InlineKeyboardButton, InlineKeyboardMarkup, InlineQuery, InlineQueryResultArticle, InputMessageContent, InputMessageContentText, Message, MessageId, ParseMode, ReplyMarkup, User, UserId};
use ChatIdKind::ID;
use ChatIdPartiality::{Both, Specific};
use crate::handlers::{ensure_lang_code, HandlerResult, reply_html, utils};
use crate::{metrics, repo};
use crate::repo::{ChatIdFull, ChatIdKind, ChatIdPartiality, Repositories};

const CALLBACK_PREFIX: &str = "pvp:";

#[derive(BotCommands, Clone, Copy)]
#[command(rename_rule = "lowercase")]
pub enum BattleCommands {
    #[command(description = "pvp")]
    PVP(u32),
    Battle(u32),
    Attack(u32),
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum BattleCommandsNoArgs {
    PVP,
    Battle,
    Attack,
}

impl BattleCommands {
    fn bet(&self) -> u32 {
        match self.clone() {
            Self::Battle(bet) => bet,
            Self::PVP(bet) => bet,
            Self::Attack(bet) => bet,
        }
    }
}

pub async fn cmd_handler(bot: Bot, msg: Message, cmd: BattleCommands, repos: Repositories) -> HandlerResult {
    metrics::CMD_PVP_COUNTER.chat.inc();

    let user = msg.from().ok_or(anyhow!("no FROM field in the PVP command handler"))?.into();
    let chat_id = msg.chat.id.into();
    let lang_code = ensure_lang_code(msg.from());
    let params = BattleParams {
        repos,
        chat_id: &chat_id,
        lang_code,
    };
    let (text, keyboard) = pvp_impl_start(params, user, cmd.bet()).await?;

    let mut answer = reply_html(bot, msg, text);
    answer.reply_markup = keyboard.map(|k| ReplyMarkup::InlineKeyboard(k));
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

    let title = t!("inline.results.titles.pvp", bet = bet, locale = &lang_code);
    let text = t!("commands.pvp.results.start", name = name, bet = bet, locale = &lang_code);
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

#[derive(Debug)]
enum EditMessageTextId {
    MessageId(MessageId),
    InlineMessageId(String),
    Invalid,
}

impl From<MessageId> for EditMessageTextId {
    fn from(value: MessageId) -> Self {
        Self::MessageId(value)
    }
}

impl From<Option<String>> for EditMessageTextId {
    fn from(value: Option<String>) -> Self {
        match value {
            Some(id) => Self::InlineMessageId(id),
            None => Self::Invalid
        }
    }
}

pub async fn callback_handler(bot: Bot, query: CallbackQuery, repos: Repositories) -> HandlerResult {
    let (chat_id, msg_id): (ChatIdPartiality, EditMessageTextId) = query.message
        .map(|msg| (msg.chat.id, msg.id.into()))
        .or_else(|| query.inline_message_id.as_ref()
            .and_then(|msg_id| utils::resolve_inline_message_id(msg_id)
                .or_else(|e| {
                    log::error!("couldn't resolve inline_message_id: {e}");
                    Err(e)
                })
                .ok())
            .map(|info| (ChatId(info.chat_id), MessageId(info.message_id).into()))
        )
        .map(|(chat_id, maybe_msg_id)| (chat_id.into(), maybe_msg_id))
        .unwrap_or((query.chat_instance.into(), query.inline_message_id.into()));

    let params = BattleParams {
        repos,
        lang_code: ensure_lang_code(Some(&query.from)),
        chat_id: &chat_id,
    };
    let (initiator, bet) = parse_data(query.data)?;
    if initiator == query.from.id {
        bot.answer_callback_query(query.id)
            .show_alert(true)
            .text(t!("commands.pvp.errors.same_person", locale = &params.lang_code))
            .await?;
        return Ok(())
    }

    let (text, keyboard) = pvp_impl_attack(params, initiator, query.from.into(), bet).await?;
    let answer_req_fut = bot.answer_callback_query(query.id).into_future();
    let (answer_resp, edit_resp) = match (chat_id, msg_id) {
        (Both(ChatIdFull { id, .. }) | Specific(ID(id)), EditMessageTextId::MessageId(msg_id)) => {
            let mut edit_req = bot.edit_message_text(id, msg_id, text);
            edit_req.parse_mode.replace(ParseMode::Html);
            edit_req.reply_markup = keyboard;
            join(
                answer_req_fut,
                edit_req.into_future().map_ok(|_| ())
            ).await
        }
        (_, EditMessageTextId::InlineMessageId(inline_msg_id)) => {
            let mut edit_req = bot.edit_message_text_inline(inline_msg_id, text);
            edit_req.parse_mode.replace(ParseMode::Html);
            edit_req.reply_markup = keyboard;
            join(
                answer_req_fut,
                edit_req.into_future().map_ok(|_| ())
            ).await
        }
        (c, m) => Err(format!("unexpected state of the query: ({c:?}, {m:?})"))?
    };
    answer_resp?;
    edit_resp?;
    metrics::CMD_PVP_COUNTER.inline.inc();
    Ok(())
}

pub(crate) struct BattleParams<'a> {
    repos: Repositories,
    chat_id: &'a ChatIdPartiality,
    lang_code: String,
}

#[derive(Clone)]
pub(crate) struct UserInfo {
    uid: UserId,
    name: String,
}

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

impl Into<UserId> for UserInfo {
    fn into(self) -> UserId {
        self.uid
    }
}

pub(crate) async fn pvp_impl_start<'a>(p: BattleParams<'a>, initiator: UserInfo, bet: u32) -> anyhow::Result<(String, Option<InlineKeyboardMarkup>)> {
    let enough = p.repos.dicks.check_dick(&p.chat_id.kind(), initiator.uid, bet).await?;
    let data = if enough {
        let text = t!("commands.pvp.results.start", name = initiator.name, bet = bet, locale = &p.lang_code);
        let btn_label = t!("commands.pvp.button", locale = &p.lang_code);
        let btn_data = format!("{CALLBACK_PREFIX}{}:{bet}", initiator.uid);
        let keyboard = InlineKeyboardMarkup::new(vec![vec![
            InlineKeyboardButton::callback(btn_label, btn_data)
        ]]);
        (text, Some(keyboard))
    } else {
        (t!("commands.pvp.errors.not_enough", locale = &p.lang_code), None)
    };
    Ok(data)
}

async fn pvp_impl_attack<'a>(p: BattleParams<'a>, initiator: UserId, acceptor: UserInfo, bet: u32) -> anyhow::Result<(String, Option<InlineKeyboardMarkup>)> {
    let enough = p.repos.dicks.check_dick(&p.chat_id.kind(), initiator, bet).await?;
    let text = if enough {
        let acceptor_uid = acceptor.clone().into();
        let (winner, loser) = choose_winner(initiator, acceptor_uid);
        let (loser_res, winner_res) = p.repos.dicks.move_length(p.chat_id, loser, winner, bet).await?;

        let winner_info = if winner == acceptor.uid {
            repo::User {
                uid: acceptor.uid.0 as i64,
                name: acceptor.name,
                created_at: Default::default(),
            }
        } else {
            p.repos.users.get(winner).await?
                .ok_or(anyhow!("winner must present in the database!"))?
        };
        let main_part = t!("commands.pvp.results.finish", locale = &p.lang_code,
            winner_name = winner_info.name, winner_length = winner_res.new_length, loser_length = loser_res.new_length);
        if let (Some(winner_pos), Some(loser_pos)) = (winner_res.pos_in_top, loser_res.pos_in_top) {
            let winner_pos = t!("commands.pvp.results.position.winner", pos = winner_pos, locale = &p.lang_code);
            let loser_pos = t!("commands.pvp.results.position.loser", pos = loser_pos, locale = &p.lang_code);
            format!("{main_part}\n\n{winner_pos}\n{loser_pos}")
        } else {
            main_part
        }
    } else {
        t!("commands.pvp.errors.not_enough", locale = &p.lang_code)
    };
    Ok((text, None))
}

fn choose_winner<T>(initiator: T, acceptor: T) -> (T, T) {
    if OsRng::default().gen_bool(0.5) {
        (acceptor, initiator)
    } else {
        (initiator, acceptor)
    }
}

fn parse_data(maybe_data: Option<String>) -> anyhow::Result<(UserId, u32)> {
    let parts = maybe_data
        .and_then(|data| data.strip_prefix(CALLBACK_PREFIX).map(|s| s.to_owned()))
        .map(|data| data.split(":").map(|s| s.to_owned()).collect::<Vec<String>>())
        .ok_or(anyhow!("callback data must be present!"))?;
    if parts.len() == 2 {
        let uid: u64 = parts[0].parse()?;
        let bet: u32 = parts[1].parse()?;
        Ok((UserId(uid), bet))
    } else {
        Err(anyhow!("invalid number of arguments ({}) in the callback data: {:?}", parts.len(), parts))
    }
}