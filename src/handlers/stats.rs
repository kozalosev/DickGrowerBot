use anyhow::anyhow;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::handlers::{ensure_lang_code, FromRefs, HandlerResult, reply_html};
use crate::{metrics, repo};
use crate::config::{AppConfig, BattlesFeatureToggles};
use crate::repo::WinRateAware;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum StatsCommands {
    #[command(description = "stats")]
    Stats
}

pub async fn cmd_handler(bot: Bot, msg: Message, repos: repo::Repositories, app_config: AppConfig) -> HandlerResult {
    metrics::CMD_STATS.chat.inc();
    
    let features = app_config.features.pvp;
    if features.show_stats {
        let from = msg.from().ok_or(anyhow!("unexpected absence of a FROM field"))?;
        let chat_id = msg.chat.id.into();
        let from_refs = FromRefs(from, &chat_id);
        let answer = stats_impl(&repos, from_refs, features).await?;

        reply_html(bot, msg, answer).await?;
    } else {
        log::info!("ignoring the /stats command since it's disabled");
    }
    Ok(())
}

pub(crate) async fn stats_impl(repos: &repo::Repositories, from_refs: FromRefs<'_>, features: BattlesFeatureToggles) -> anyhow::Result<String> {
    let lang_code = ensure_lang_code(Some(from_refs.0));
    let (length, position) = repos.dicks.fetch_dick(from_refs.0.id, &from_refs.1.kind()).await?
        .map(|dick| (dick.length, dick.position.unwrap_or_default()))
        .unwrap_or_default();
    let length_stats = t!("commands.stats.length", locale = &lang_code,
        length = length, pos = position);
    let pvp_stats = repos.pvp_stats.get_stats(&from_refs.1.kind(), from_refs.0.id).await
        .map(|stats| t!("commands.stats.pvp", locale = &lang_code,
            win_rate = stats.win_rate_formatted(), win_streak = stats.win_streak_max,
            battles = stats.battles_total, wins = stats.battles_won,
            acquired = stats.acquired_length, lost = stats.lost_length))
        .map(|s| if features.show_stats_notice {
            let notice = t!("commands.stats.notice", locale = &lang_code);
            format!("{}\n\n<i>{}</i>", s, notice)
        } else {
            s
        })?;
    Ok(format!("{length_stats}\n\n{pvp_stats}"))
}
