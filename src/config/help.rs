use teloxide::types::Me;
use crate::handlers::perks::HelpPussiesPerk;
use crate::handlers::utils::Incrementor;
use crate::help;

pub fn build_context_for_help_messages(me: Me, incr: &Incrementor, competitor_bots: &[&str]) -> anyhow::Result<help::Context> {
    let other_bots = competitor_bots
        .iter()
        .map(|username| ensure_starts_with_at_sign(username.to_string()))
        .collect::<Vec<String>>()
        .join(", ");
    let incr_cfg = incr.get_config();

    Ok(help::Context {
        bot_name: me.username().to_owned(),
        grow_min: incr_cfg.growth_range_min().to_string(),
        grow_max: incr_cfg.growth_range_max().to_string(),
        other_bots,
        help_pussies_percentage: incr.find_perk_config::<HelpPussiesPerk>()
            .map(|payout_ratio| payout_ratio * 100.0)
            .unwrap_or(0.0)
    })
}

fn ensure_starts_with_at_sign(s: String) -> String {
    if s.starts_with('@') {
        s
    } else {
        format!("@{s}")
    }
}

#[cfg(test)]
mod test {
    use super::ensure_starts_with_at_sign;

    #[test]
    fn test_ensure_starts_with_at_sign() {
        let result = "@test";
        assert_eq!(ensure_starts_with_at_sign("test".to_owned()), result);
        assert_eq!(ensure_starts_with_at_sign("@test".to_owned()), result);
    }
}
