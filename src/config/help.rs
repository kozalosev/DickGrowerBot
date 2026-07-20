use teloxide::types::Me;
use crate::config::env::get_env_mandatory_value;
use crate::domain::primitives::{Percentage, Username};
use crate::handlers::perks::HelpPussiesPerk;
use crate::handlers::utils::Incrementor;
use crate::help;

pub fn build_context_for_help_messages(me: Me, incr: &Incrementor, competitor_bots: &[&str]) -> anyhow::Result<help::Context> {
    let other_bots = competitor_bots
        .iter()
        .map(|username| Username::new(username.to_string()).value_with_at_sign())
        .collect::<Vec<String>>()
        .join(", ");
    let incr_cfg = incr.get_config();

    Ok(help::Context {
        bot_name: Username::from(me.username()),
        grow_min: incr_cfg.growth_range_min().to_string(),
        grow_max: incr_cfg.growth_range_max().to_string(),
        other_bots,
        admin_channel_ru: get_env_mandatory_value("HELP_ADMIN_CHANNEL_RU")?,
        admin_channel_en: get_env_mandatory_value("HELP_ADMIN_CHANNEL_EN")?,
        admin_chat_ru: get_env_mandatory_value("HELP_ADMIN_CHAT_RU")?,
        admin_chat_en: get_env_mandatory_value("HELP_ADMIN_CHAT_EN")?,
        git_repo: get_env_mandatory_value("HELP_GIT_REPO")?,
        help_pussies_percentage: incr.find_perk_config::<HelpPussiesPerk>()
            .map(Percentage::from)
            .unwrap_or(Percentage::literal(0))
    })
}
