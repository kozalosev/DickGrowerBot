use std::ops::RangeInclusive;
use std::str::FromStr;
use crate::config::env::{env_value, get_env_value_or_default, parse_sorted_numbers};
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
    pub streak_grades: StreakGrades,
}

/// The days of a streak on which its bonus grows by a centimetre, sorted and without repetitions.
///
/// A day below the second is dropped when the variable is read: every streak is at least a day
/// long, so such a grade would be a flat bonus for everybody rather than a reward for coming back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StreakGrades(Vec<DaysCount>);

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

/// Each grade takes twice as long to reach as the one before it.
impl Default for StreakGrades {
    fn default() -> Self {
        Self([2, 4, 8, 16, 32].map(DaysCount::new).into())
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
                streak_grades: get_env_value_or_default("STREAK_BONUS_GRADES", defaults.perks.streak_grades),
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

impl StreakGrades {
    /// How many grades a streak of this many days has reached.
    pub fn reached(&self, streak: DaysCount) -> usize {
        self.0.partition_point(|day| *day <= streak)
    }

    /// The day the next grade starts on, unless the last one is already reached.
    pub fn next(&self, streak: DaysCount) -> Option<DaysCount> {
        self.0.get(self.reached(streak)).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl FromStr for StreakGrades {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let days = parse_sorted_numbers(s)?
            .into_iter()
            .filter(|value| {
                let acceptable = *value >= 2;
                if !acceptable {
                    tracing::warn!(day = %value, "a streak grade is dropped: it starts before the second day");
                }
                acceptable
            })
            .map(DaysCount::new)
            .collect();
        Ok(Self(days))
    }
}

impl std::fmt::Display for StreakGrades {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let values = self.0.iter()
            .map(|day| day.to_string())
            .collect::<Vec<_>>();
        f.write_str(&values.join(", "))
    }
}

#[cfg(test)]
mod test {
    use crate::config::StreakGrades;
    use crate::domain::primitives::DaysCount;

    #[test]
    fn grades_are_sorted_deduped_and_start_on_the_second_day() {
        let grades: StreakGrades = "8, 2,4,,2,1,0".parse().expect("couldn't parse the grades");
        assert_eq!(grades.to_string(), "2, 4, 8");

        let empty: StreakGrades = "".parse().expect("couldn't parse the grades");
        assert!(empty.is_empty());

        assert!("2,soon".parse::<StreakGrades>().is_err());
    }

    #[test]
    fn a_streak_reaches_every_grade_up_to_its_length() {
        let grades = StreakGrades::default();
        let at = |streak| (grades.reached(DaysCount::new(streak)), grades.next(DaysCount::new(streak)));

        assert_eq!(at(1), (0, Some(DaysCount::new(2))));
        assert_eq!(at(3), (1, Some(DaysCount::new(4))));
        assert_eq!(at(4), (2, Some(DaysCount::new(8))));
        assert_eq!(at(32), (5, None));
        assert_eq!(at(100), (5, None));
    }
}
