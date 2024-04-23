use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;
use async_trait::async_trait;
use derive_more::{AddAssign, Display};
use num_traits::Num;
use rand::distributions::uniform::SampleUniform;
use rand::Rng;
use rand::rngs::OsRng;
use rust_i18n::t;
use teloxide::types::UserId;
use crate::{config, repo};
use crate::repo::ChatIdKind;

#[derive(Clone)]
pub struct Incrementor {
    config: Config,
    perks: Vec<Arc<dyn Perk>>,
    dicks: repo::Dicks,
}

#[derive(Clone)]
pub struct Config {
    growth_range: RangeInclusive<i32>,
    grow_shrink_ratio: f32,
    newcomers_grace_days: u32,
    dod_bonus_range: RangeInclusive<u16>,
}

#[async_trait]
pub trait Perk: Send + Sync {
    fn name(&self) -> &str;
    fn enabled(&self) -> bool;
    async fn apply(&self, dick_id: &DickId, change_intent: ChangeIntent) -> AdditionalChange;
}

#[derive(Display, Clone, Hash, PartialEq)]
#[display("(user_id={_0}, chat_id={_1}")]
pub struct DickId(pub(crate) UserId, pub(crate) ChatIdKind);

#[derive(Copy, Clone)]
pub struct ChangeIntent {
    pub current_length: i32,
    pub base_increment: i32,
}

#[derive(Copy, Clone, AddAssign)]
pub struct AdditionalChange(pub i32);

pub struct Increment<T: Num + Copy + std::fmt::Display> {
    pub base: T,
    pub by_perks: HashMap<String, i32>,
    pub total: T,
}

pub type SignedIncrement = Increment<i32>;
pub type UnsignedIncrement = Increment<u16>;

impl Config {
    pub fn growth_range_min(&self) -> i32 {
        self.growth_range.clone()
            .min()
            .unwrap_or(0)
    }

    pub fn growth_range_max(&self) -> i32 {
        self.growth_range.clone()
            .max()
            .unwrap_or(0)
    }
}

impl Incrementor {
    pub fn from_env(dicks: &repo::Dicks, perks: Vec<Box<dyn Perk>>) -> Self {
        let growth_range_min = config::get_env_value_or_default("GROWTH_MIN", -5);
        let growth_range_max = config::get_env_value_or_default("GROWTH_MAX", 10);
        let dod_max_bonus = config::get_env_value_or_default("GROWTH_DOD_BONUS_MAX", 5);
        
        let perks = perks
            .into_iter()
            .filter(|perk| perk.enabled())
            .map(Arc::from)
            .collect();

        Self {
            config: Config {
                growth_range: growth_range_min..=growth_range_max,
                grow_shrink_ratio: config::get_env_value_or_default("GROW_SHRINK_RATIO", 0.5),
                newcomers_grace_days: config::get_env_value_or_default("NEWCOMERS_GRACE_DAYS", 7),
                dod_bonus_range: 1..=dod_max_bonus,
            },
            perks,
            dicks: dicks.clone(),
        }
    }

    pub fn get_config(&self) -> Config {
        self.config.clone()
    }

    #[cfg(test)]
    fn set_perks(&mut self, perks: Vec<Box<dyn Perk>>) {
        self.perks = perks.into_iter()
            .map(Arc::from)
            .collect();
    }

    pub async fn growth_increment(&self, user_id: UserId, chat_id: ChatIdKind, days_since_registration: u32) -> SignedIncrement {
        let dick_id = DickId(user_id, chat_id);
        let grow_shrink_ratio = if days_since_registration > self.config.newcomers_grace_days {
            self.config.grow_shrink_ratio
        } else {
            1.0
        };
        let base_incr = get_base_increment(self.config.growth_range.clone(), grow_shrink_ratio);
        self.add_additional_incr(dick_id, base_incr).await
    }

    pub async fn dod_increment(&self, user_id: UserId, chat_id: ChatIdKind) -> UnsignedIncrement {
        let dick_id = DickId(user_id, chat_id);
        let base_incr = OsRng.gen_range(self.config.dod_bonus_range.clone());
        self.add_additional_incr(dick_id, base_incr).await
    }

    async fn add_additional_incr<T>(&self, dick: DickId, base_increment: T) -> Increment<T>
    where
        T: Num + Copy + std::fmt::Display + Into<i32> + TryFrom<i32>,
        <T as TryFrom<i32>>::Error: std::fmt::Debug
    {
        let current_length = match self.dicks.fetch_length(dick.0, &dick.1).await {
            Ok(length) => length,
            Err(e) => {
                log::error!("couldn't fetch the length of a dick: {e}");
                return Increment::base_only(base_increment)
            }
        };
        let change_intent = ChangeIntent {
            base_increment: base_increment.into(),
            current_length
        };

        let mut additional_change = AdditionalChange(0);
        let mut by_perks = HashMap::new();
        for perk in self.perks.iter() {
            let ac = perk.apply(&dick, change_intent).await;
            by_perks.insert(perk.name().to_owned(), ac.0);
            additional_change += ac
        }
        Increment {
            base: base_increment,
            by_perks,
            total: T::try_from(change_intent.base_increment + additional_change.0).expect("TODO: fix numeric types!")
        }
    }
}

impl <T: Num + Copy + std::fmt::Display> Increment<T> {
    fn base_only(base: T) -> Self {
        Self {
            base,
            total: base,
            by_perks: HashMap::default(),
        }
    }
    
    pub fn perks_part_of_answer(&self, lang_code: &str) -> String {
        if self.base != self.total {
            let top_line = t!("titles.perks.top_line", locale = lang_code);
            let perks = self.by_perks.iter()
                .map(|(perk, value)| {
                    let name = t!(perk, locale = lang_code);
                    format!("— {name} ({value})")
                })
                .collect::<Vec<String>>()
                .join("\n");
            format!("\n{top_line}:\n{perks}")
        } else {
            String::default()
        }
    }
}

fn get_base_increment<T>(range: RangeInclusive<T>, sign_ratio: f32) -> T
where
    T: Num + Copy + PartialOrd + SampleUniform + From<i32>
{
    let sign_ratio_percent = match (sign_ratio * 100.0).round() as u32 {
        ..=0 => 0,
        100.. => 100,
        x => x
    };
    let mut rng = OsRng;
    let zero = T::from(0);
    if range.start() > &zero {
        return rng.gen_range(range)
    }
    let positive = rng.gen_ratio(sign_ratio_percent, 100);
    if positive {
        let end = *range.end();
        let one = T::from(1);
        rng.gen_range(one..=end)
    } else {
        let start = *range.start();
        let minus_one = T::from(-1);
        rng.gen_range(start..=minus_one)
    }
}

#[cfg(test)]
mod test {
    use super::get_base_increment;

    #[test]
    fn test_gen_increment() {
        let increments: Vec<i32> = (0..100)
            .map(|_| get_base_increment(-5..=10, 0.5))
            .collect();
        assert!(increments.iter().any(|n| n > &0));
        assert!(increments.iter().any(|n| n < &0));
        assert!(increments.iter().all(|n| n != &0));
        assert!(increments.iter().all(|n| n <= &10));
        assert!(increments.iter().all(|n| n >= &-5));
    }

    #[test]
    fn test_gen_increment_with_positive_range() {
        let increments: Vec<i32> = (0..100)
            .map(|_| get_base_increment(5..=10, 0.5))
            .collect();
        assert!(increments.iter().all(|n| n <= &10));
        assert!(increments.iter().all(|n| n >= &5));
    }
}

#[cfg(test)]
mod test_incrementor {
    use std::iter::zip;

    use async_trait::async_trait;
    use futures::future::join_all;
    use testcontainers::clients;

    use crate::handlers::utils::{AdditionalChange, ChangeIntent, Config, DickId, Incrementor, Perk};
    use crate::repo;
    use crate::repo::test::{CHAT_ID_KIND, start_postgres, USER_ID};

    #[tokio::test]
    async fn test_incrementor() {
        let docker = clients::Cli::default();
        let (_container, db) = start_postgres(&docker).await;
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let incr = Incrementor {
            config: Config {
                growth_range: -1..=1,
                grow_shrink_ratio: 0.5,
                newcomers_grace_days: 1,
                dod_bonus_range: 1..=2,
            },
            dicks,
            perks: Vec::default()
        };

        test_growth_increment_base(&incr).await;
        test_dod_increment_base(&incr).await;
        test_with_perks(&incr).await;
    }

    async fn test_growth_increment_base(incr: &Incrementor) {
        let lazy_vals = (0..100)
            .map(|_| incr.growth_increment(USER_ID, CHAT_ID_KIND, 1));
        for fut in lazy_vals {
            let val = fut.await;
            assert_eq!(val.base, val.total);
            assert_ne!(val.base, 0);
            assert!(val.base >= -1);
            assert!(val.base <= 1);
        }

        let lazy_positive_vals = (0..100)
            .map(|_| incr.growth_increment(USER_ID, CHAT_ID_KIND, 0));
        for fut in lazy_positive_vals {
            let val = fut.await;
            assert_eq!(val.base, val.total);
            assert!(val.base > 0);
        }
    }

    async fn test_dod_increment_base(incr: &Incrementor) {
        let val = (0..100)
            .map(|_| incr.dod_increment(USER_ID, CHAT_ID_KIND));
        let val = join_all(val).await;
        assert!(val.iter().all(|n| { n.base == n.total }));
        assert!(val.iter().all(|n| { n.base == 1 || n.base == 2 }))
    }

    #[derive(Clone)]
    struct AddPerk {
        value: i32,
        name: String,
    }

    impl AddPerk {
        fn boxed(value: i32) -> Box<Self> {
            Box::new(Self {
                value,
                name: format!("add-perk-{value}")
            })
        }
    }

    #[async_trait]
    impl Perk for AddPerk {
        fn name(&self) -> &str {
            &self.name
        }

        fn enabled(&self) -> bool {
            true
        }

        async fn apply(&self, _: &DickId, _: ChangeIntent) -> AdditionalChange {
            AdditionalChange(self.value)
        }
    }

    async fn test_with_perks(incr: &Incrementor) {
        let mut incr = incr.clone();
        let perk_plus2 = AddPerk::boxed(2);
        let perk_minus1 = AddPerk::boxed(-1);
        incr.set_perks(vec![perk_plus2.clone(), perk_minus1.clone()]);

        let growth_lazy_vals = (0..100)
            .map(|_| incr.growth_increment(USER_ID, CHAT_ID_KIND, 1));
        let dod_lazy_vals = (0..100)
            .map(|_| incr.dod_increment(USER_ID, CHAT_ID_KIND));

        macro_rules! assertions {
            ($val:ident) => {
                assert_eq!($val.total - $val.base, 1);
                assert_eq!($val.by_perks[perk_plus2.name()], 2);
                assert_eq!($val.by_perks[perk_minus1.name()], -1);
            };
        }

        for (growth_fut, dod_fut) in zip(growth_lazy_vals, dod_lazy_vals) {
            let (growth_val, dod_val) = (growth_fut.await, dod_fut.await);
            assertions!(growth_val);
            assertions!(dod_val);
        }
    }
}
