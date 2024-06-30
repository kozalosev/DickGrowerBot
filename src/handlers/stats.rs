use anyhow::anyhow;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::handlers::{ensure_lang_code, FromRefs, HandlerResult, reply_html};
use crate::{metrics, repo};
use crate::repo::WinRateAware;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum StatsCommands {
    #[command(description = "stats")]
    Stats
}

pub async fn cmd_handler(bot: Bot, msg: Message, repos: repo::Repositories) -> HandlerResult {
    metrics::CMD_STATS.chat.inc();

    let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
    let chat_id = msg.chat.id.into();
    let from_refs = FromRefs(from, &chat_id);
    let answer = stats_impl(&repos, from_refs).await?;

    reply_html(bot, msg, answer).await?;
    Ok(())
}

pub(crate) async fn stats_impl(repos: &repo::Repositories, from_refs: FromRefs<'_>) -> anyhow::Result<String> {
    let lang_code = ensure_lang_code(Some(from_refs.0));
    repos.pvp_stats.get_stats(&from_refs.1.kind(), from_refs.0.id).await
        .map(|stats| t!("commands.stats.result", locale = &lang_code,
            win_rate = stats.win_rate_formatted(), win_streak = stats.win_streak_max,
            battles = stats.battles_total, wins = stats.battles_won,
            acquired = stats.acquired_length, lost = stats.lost_length))
}
