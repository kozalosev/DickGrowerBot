use std::ops::RangeInclusive;
use crate::config::env::{env_value, get_env_value_or_default};
use crate::domain::primitives::{DaysCount, PerkName, Ratio};
use domain_types::literal;

/// Tuning of the length changes produced by the incrementor and its perks.
#[derive(Clone)]
pub struct IncrementorConfig {
    pub growth_range: RangeInclusive<i16>,
    pub grow_shrink_ratio: Ratio,
    pub newcomers_grace_days: DaysCount,
    pub dod_bonus_range: RangeInclusive<u8>,
    pub perks: PerksConfig,
}

#[derive(Clone, Default)]
pub struct PerksConfig {
    /// Zero by default, which leaves `help-pussies` off until someone asks for it.
    pub help_pussies_ratio: Ratio,
    pub streak_bonus: StreakBonusConfig,
}

/// How much a day in a row is worth, and how many of them still count.
#[derive(Clone, Copy)]
pub struct StreakBonusConfig {
    pub ratio_per_day: Ratio,
    pub max_days: DaysCount,
}

impl Default for IncrementorConfig {
    fn default() -> Self {
        Self {
            growth_range: -5..=10,
            grow_shrink_ratio: literal!(Ratio = 0.5),
            newcomers_grace_days: DaysCount::new(7),
            dod_bonus_range: 1..=5,
            perks: Default::default(),
        }
    }
}

impl Default for StreakBonusConfig {
    fn default() -> Self {
        Self {
            ratio_per_day: literal!(Ratio = 0.05),
            max_days: DaysCount::new(20),
        }
    }
}

impl IncrementorConfig {
    pub(super) fn from_env() -> Self {
        let defaults = Self::default();
        let growth_range_min = get_env_value_or_default("GROWTH_MIN", *defaults.growth_range.start());
        let growth_range_max = get_env_value_or_default("GROWTH_MAX", *defaults.growth_range.end());
        let dod_max_bonus = get_env_value_or_default("GROWTH_DOD_BONUS_MAX", *defaults.dod_bonus_range.end());

        Self {
            growth_range: growth_range_min..=growth_range_max,
            grow_shrink_ratio: get_env_value_or_default("GROW_SHRINK_RATIO", defaults.grow_shrink_ratio),
            newcomers_grace_days: get_env_value_or_default("NEWCOMERS_GRACE_DAYS", defaults.newcomers_grace_days),
            dod_bonus_range: *defaults.dod_bonus_range.start()..=dod_max_bonus,
            perks: PerksConfig {
                help_pussies_ratio: env_value!("HELP_PUSSIES_COEF": Ratio),
                streak_bonus: StreakBonusConfig {
                    ratio_per_day: get_env_value_or_default("STREAK_BONUS_RATIO_PER_DAY",
                        defaults.perks.streak_bonus.ratio_per_day),
                    max_days: get_env_value_or_default("STREAK_BONUS_MAX_DAYS",
                        defaults.perks.streak_bonus.max_days),
                },
            },
        }
    }

    pub fn growth_range_min(&self) -> i16 {
        self.growth_range.clone()
            .min()
            .unwrap_or(0)
    }

    pub fn growth_range_max(&self) -> i16 {
        self.growth_range.clone()
            .max()
            .unwrap_or(0)
    }
}

impl PerksConfig {
    /// Every perk can be turned off by a `DISABLE_<NAME>` variable named after the perk.
    pub fn enabled(&self, perk_name: &PerkName) -> bool {
        let env_key = format!("DISABLE_{}", perk_name.value().to_uppercase().replace('-', "_"));
        !get_env_value_or_default(&env_key, false)
    }
}
