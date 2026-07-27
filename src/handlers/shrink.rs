use rust_i18n::t;
use crate::config::AppConfig;
use crate::domain::primitives::LanguageCode;
use crate::handlers::FromRefs;
use crate::repo::Repositories;

/// Builds the inline `shrinks` command reply: shrink events of the last `shrink_events_days` days,
/// newest first. Mirrors the shape of [`crate::handlers::stats::chat_stats_impl`].
pub(crate) async fn recent_shrinks_impl(
    repos: &Repositories,
    from_refs: FromRefs<'_>,
    config: &AppConfig,
    lang_code: &LanguageCode,
) -> anyhow::Result<String> {
    let shrinks = repos.shrinks
        .get_recent_shrinks(&from_refs.1.kind(), config.stale_dicks_shrinking.history_window_days)
        .await?;
    if shrinks.is_empty() {
        return Ok(t!("commands.shrink.empty", locale = lang_code).to_string());
    }

    let lines = shrinks.iter()
        .map(|s| {
            let name = s.owner_name.escaped();
            t!("commands.shrink.recent.line", locale = lang_code, name = name, lost = s.lost_length)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let header = t!("commands.shrink.recent.header", locale = lang_code);
    Ok(format!("{header}\n{lines}"))
}
