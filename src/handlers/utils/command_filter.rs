use teloxide::dispatching::DpHandlerDescription;
use teloxide::dptree;
use teloxide::dptree::Handler;
use teloxide::requests::Requester;
use teloxide::types::{ChatId, Me, Message};
use teloxide::utils::command::BotCommands;
use crate::config::ThrottledBot;
use crate::domain::objects::BotNames;
use crate::domain::primitives::Username;
use crate::errors::flat::flat;

/// The command filter that knows every username of the bot, in place of teloxide's
/// `filter_command`, which knows only the one `getMe` returns.
pub trait BotCommandFilterExt {
    #[must_use]
    fn filter_bot_command<C>(self) -> Self
    where
        C: BotCommands + Send + Sync + 'static;
}

/// The names from `getChat` on the bot itself, or `getMe`'s alone when that fails.
pub async fn fetch_bot_names(bot: &ThrottledBot, me: &Me) -> BotNames {
    let others = bot.get_chat(ChatId::from(me.id))
        .await
        .inspect_err(|e| tracing::warn!(error = flat!(e), "couldn't read the bot's usernames"))
        .ok()
        .and_then(|chat| chat.active_usernames().map(<[String]>::to_vec))
        .unwrap_or_default();
    let names = BotNames::new(Username::from(me.username()), others.into_iter().map(Username::new));
    tracing::info!(%names, "the bot answers to its usernames");
    names
}

impl<Output> BotCommandFilterExt for Handler<'static, Output, DpHandlerDescription>
where
    Output: Send + Sync + 'static,
{
    fn filter_bot_command<C>(self) -> Self
    where
        C: BotCommands + Send + Sync + 'static,
    {
        self.chain(dptree::filter_map(|msg: Message, names: BotNames| {
            msg.text()
                .or_else(|| msg.caption())
                .and_then(|text| names.parse::<C>(text))
        }))
    }
}
