use anyhow::anyhow;
use rust_i18n::t;
use teloxide::Bot;
use teloxide::macros::BotCommands;
use teloxide::prelude::Message;
use crate::handlers::{FromRefs, HandlerResult, reply_html};
use crate::{metrics, reply_html, repo};
use crate::config::{AppConfig, BattlesFeatureToggles};
use crate::domain::primitives::{LanguageCode, UserId};
use crate::domain::objects::WinRateAware;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
pub enum StatsCommands {
    #[command(description = "stats")]
    Stats
}

pub async fn cmd_handler(
    bot: Bot,
    msg: Message,
    repos: repo::Repositories,
    app_config: AppConfig,
    lang_code: LanguageCode,
) -> HandlerResult {
    metrics::CMD_STATS.chat.inc();

    let features = app_config.features.pvp;
    if features.show_stats {
        let from = msg.from.as_ref().ok_or(anyhow!("unexpected absence of a FROM field"))?;
        let chat_id = msg.chat.id.into();
        let from_refs = FromRefs(from, &chat_id);

        let answer = if msg.chat.is_private() {
            personal_stats_impl(&repos, from_refs, lang_code).await?
        } else {
            chat_stats_impl(&repos, from_refs, features, lang_code).await?
        };

        reply_html!(bot, msg, answer);
    } else {
        log::info!("ignoring the /stats command since it's disabled");
    }
    Ok(())
}

async fn personal_stats_impl(
    repos: &repo::Repositories,
    from_refs: FromRefs<'_>,
    lang_code: LanguageCode,
) -> anyhow::Result<String> {
    repos.personal_stats.get(UserId::from(from_refs.0)).await
        .map(|stats| t!("commands.stats.personal", locale = &lang_code,
            chats = stats.chats, max_length = stats.max_length, total_length = stats.total_length).to_string())
}

pub(crate) async fn chat_stats_impl(
    repos: &repo::Repositories,
    from_refs: FromRefs<'_>,
    features: BattlesFeatureToggles,
    lang_code: LanguageCode,
) -> anyhow::Result<String> {
    let (length, position) = repos.dicks.fetch_dick(UserId::from(from_refs.0), &from_refs.1.kind()).await?
        .map(|dick| (dick.length, dick.position.unwrap_or_default()))
        .unwrap_or_default();
    let length_stats = t!("commands.stats.length", locale = &lang_code,
        length = length, pos = position);
    let pvp_stats = repos.pvp_stats.get_stats(&from_refs.1.kind(), UserId::from(from_refs.0)).await
        .map(|stats| t!("commands.stats.pvp", locale = &lang_code,
            win_rate = stats.win_rate_percentage(), win_streak = stats.win_streak_max,
            battles = stats.battles_total, wins = stats.battles_won,
            acquired = stats.acquired_length, lost = stats.lost_length))
        .map(|s| if features.show_stats_notice {
            let notice = t!("commands.stats.notice", locale = &lang_code);
            format!("{}\n\n<i>{}</i>", s, notice)
        } else {
            s.to_string()
        })?;
    Ok(format!("{length_stats}\n\n{pvp_stats}"))
}
