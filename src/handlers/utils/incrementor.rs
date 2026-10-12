use std::collections::HashMap;
use std::ops::RangeInclusive;
use domain_types::traits::SaturatingInto;
use std::sync::Arc;
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::NaiveDate;
use derive_more::Display;
use downcast_rs::{Downcast, impl_downcast};
use futures::{future, stream, StreamExt};
use num_traits::PrimInt;
use rand::distr::uniform::SampleUniform;
use rand::RngExt;
use rust_i18n::t;
use sqlx::types::JsonValue;
use crate::repo;
use crate::config::IncrementorConfig;
use crate::domain::objects::{PerkStateUpdate, PerkWrites};
use crate::domain::primitives::chat::ChatIdKind;
use crate::domain::primitives::{DaysCount, LanguageCode, Length, LengthChange, LoanPayout, PerkId, PerkName, PerkNote, Ratio, SignedLengthChange, UserId};
use domain_types::literal;

const MAX_REROLLS: usize = 10;

#[derive(Clone)]
pub struct Incrementor {
    config: IncrementorConfig,
    perks: Vec<RegisteredPerk>,
    dicks: repo::Dicks,
    perk_states: repo::PerkStates,
}

#[async_trait]
pub trait Perk: Send + Sync + Downcast {
    fn name(&self) -> PerkName;
    async fn apply(&self, ctx: PerkContext<'_>) -> PerkOutcome;

    /// What the perk has to say about its owner in `/stats`. Nothing, unless it says otherwise.
    fn stats_line(&self, _state: Option<&JsonValue>, _lang_code: &LanguageCode) -> Option<String> {
        None
    }

    fn enabled(&self) -> bool {
        true
    }
}
impl_downcast!(Perk);

pub trait ConfigurablePerk: Perk {
    type Config;

    fn get_config(&self) -> Self::Config;
}

#[derive(Display, Clone, Hash, PartialEq)]
#[display("(user_id={_0}, chat_id={_1})")]
pub struct DickId(pub(crate) UserId, pub(crate) ChatIdKind);

#[derive(Copy, Clone)]
pub struct ChangeIntent {
    pub current_length: Length,
    pub base_increment: LengthChange,
}

/// What the change is for. A Dick of the Day award is not a growth, however much it looks like one
/// from the length's point of view, so a perk about playing every day must be able to tell them
/// apart.
#[derive(Copy, Clone, PartialEq)]
pub enum ChangeSource {
    Growth,
    DickOfDay,
}

/// Everything a perk is given: the change it may alter, what it stored the last time, and the day
/// the database is having.
pub struct PerkContext<'a> {
    pub dick_id: &'a DickId,
    pub intent: ChangeIntent,
    pub source: ChangeSource,
    pub state: Option<&'a JsonValue>,
    pub today: NaiveDate,
    pub lang_code: &'a LanguageCode,
}

/// What a perk made of it. The state travels with the change instead of being stored on the spot,
/// so it is written in the transaction that writes the length — or not at all.
pub struct PerkOutcome {
    pub change: AdditionalChange,
    pub state: Option<JsonValue>,
    /// Extra detail for the "perks affected the result" line, e.g. how long a streak has grown to.
    /// Shown only next to a non-zero change.
    pub note: Option<PerkNote>,
    /// Taken off the loan once the change is written, not while the perk is being applied.
    pub loan_payout: Option<LoanPayout>,
}

impl From<AdditionalChange> for PerkOutcome {
    fn from(change: AdditionalChange) -> Self {
        Self { change, state: None, note: None, loan_payout: None }
    }
}

#[derive(Copy, Clone)]
pub struct AdditionalChange(pub LengthChange);

impl AdditionalChange {
    pub fn zero() -> Self {
        Self(LengthChange::signed(0))
    }
}

/// What one perk did to the roll, for the "perks affected the result" line.
#[derive(Debug)]
pub struct PerkEffect {
    pub change: SignedLengthChange,
    pub note: Option<PerkNote>,
}

pub struct Increment {
    pub base: LengthChange,
    pub by_perks: HashMap<PerkName, PerkEffect>,
    pub total: LengthChange,
    pub writes: PerkWrites,
}

#[derive(Clone)]
struct RegisteredPerk {
    id: PerkId,
    perk: Arc<dyn Perk>,
}

type BaseIncrement = SignedLengthChange;

impl BaseIncrement {
    fn only(self) -> Increment {
        Increment::of_base(LengthChange::from(self))
    }
}

impl Incrementor {
    pub async fn new(
        config: IncrementorConfig,
        dicks: &repo::Dicks,
        perk_states: &repo::PerkStates,
        perks: Vec<Box<dyn Perk>>,
    ) -> anyhow::Result<Self> {
        let (enabled, disabled): (Vec<_>, Vec<_>) = perks
            .into_iter()
            .partition(|perk| perk.enabled() && config.perks.enabled(&perk.name()));
        let enabled_names: Vec<PerkName> = enabled.iter().map(|perk| perk.name()).collect();
        let disabled_names: Vec<PerkName> = disabled.iter().map(|perk| perk.name()).collect();
        tracing::info!(enabled = ?enabled_names, disabled = ?disabled_names, "perks are configured");

        let ids = perk_states.register_all(&enabled_names).await?;
        let perks = enabled.into_iter()
            .map(|perk| {
                let name = perk.name();
                let id = ids.get(&name)
                    .copied()
                    .ok_or_else(|| anyhow!("the perk {name} was not given an id"))?;
                Ok(RegisteredPerk { id, perk: Arc::from(perk) })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            config,
            perks,
            dicks: dicks.clone(),
            perk_states: perk_states.clone(),
        })
    }

    pub fn get_config(&self) -> IncrementorConfig {
        self.config.clone()
    }

    pub fn find_perk_config<P: ConfigurablePerk>(&self) -> Option<P::Config> {
        self.perks.iter()
            .map(|registered| &registered.perk)
            .find(|p| p.is::<P>())
            .and_then(|p| p.downcast_ref::<P>())
            .map(ConfigurablePerk::get_config)
    }

    /// What the perks have to add to `/stats`. Their states are read once and handed out, so a
    /// perk with nothing to say costs nothing.
    pub async fn perks_stats_lines(&self, dick: &DickId, lang_code: &LanguageCode) -> Vec<String> {
        if self.perks.is_empty() {
            return Vec::default()
        }
        let Ok(snapshot) = self.perk_states.read(dick.0, &dick.1).await
            .inspect_err(|e| tracing::error!(dick = %dick, error = %e, "couldn't read the perk states for the statistics"))
        else {
            return Vec::default()
        };
        self.perks.iter()
            .filter_map(|registered| registered.perk.stats_line(snapshot.of(registered.id), lang_code))
            .collect()
    }

    #[cfg(test)]
    fn set_perks(&mut self, perks: Vec<Box<dyn Perk>>) {
        self.perks = perks.into_iter()
            .enumerate()
            .map(|(n, perk)| RegisteredPerk {
                id: PerkId::new(n.saturating_into()),
                perk: Arc::from(perk),
            })
            .collect();
    }

    pub async fn growth_increment(
        &self,
        user_id: UserId,
        chat_id: ChatIdKind,
        days_since_registration: DaysCount,
        lang_code: &LanguageCode,
    ) -> Increment {
        let dick_id = DickId(user_id, chat_id);
        let grow_shrink_ratio = if days_since_registration > self.config.newcomers_grace_days {
            self.config.grow_shrink_ratio
        } else {
            literal!(Ratio = 1.0)
        };
        self.nonzero_increment(dick_id, ChangeSource::Growth, lang_code, || {
            let base_incr = get_base_increment(self.config.growth_range.clone(), grow_shrink_ratio);
            SignedLengthChange::new(base_incr.into())
        }).await
    }

    pub async fn dod_increment(&self, user_id: UserId, chat_id: ChatIdKind, lang_code: &LanguageCode) -> Increment {
        let dick_id = DickId(user_id, chat_id);
        self.nonzero_increment(dick_id, ChangeSource::DickOfDay, lang_code, || {
            let base_incr = rand::rng().random_range(self.config.dod_bonus_range.clone());
            SignedLengthChange::new(base_incr.into())
        }).await
    }

    async fn nonzero_increment(
        &self,
        dick: DickId,
        source: ChangeSource,
        lang_code: &LanguageCode,
        roll_base: impl Fn() -> BaseIncrement + Sync,
    ) -> Increment {
        stream::repeat_with(&roll_base)
            .take(MAX_REROLLS)
            .then(|base| self.add_additional_incr(dick.clone(), base, source, lang_code))
            .inspect(|increment| if increment.total.is_zero() {
                tracing::debug!(%dick, "perks cancelled the increment, rerolling the base");
            })
            .filter(|increment| future::ready(!increment.total.is_zero()))
            .boxed()
            .next()
            .await
            .unwrap_or_else(|| {
                tracing::warn!(%dick, "perks kept cancelling the increment, skipping them");
                roll_base().only()
            })
    }

    async fn add_additional_incr(
        &self,
        dick: DickId,
        base_increment: BaseIncrement,
        source: ChangeSource,
        lang_code: &LanguageCode,
    ) -> Increment {
        let Ok(current_length) = self.dicks.fetch_length(dick.0, &dick.1).await
            .inspect_err(|e| tracing::error!(error = %e, "couldn't fetch the length of a dick"))
        else {
            return base_increment.only()
        };
        let Ok(states) = self.perk_states.read(dick.0, &dick.1).await
            .inspect_err(|e| tracing::error!(error = %e, "couldn't fetch the perk states of a dick"))
        else {
            return base_increment.only()
        };
        let base = LengthChange::from(base_increment);
        let change_intent = ChangeIntent {
            base_increment: base,
            current_length,
        };

        let mut additional_change = SignedLengthChange::new(0);
        let mut by_perks = HashMap::new();
        let mut writes = PerkWrites::default();
        for RegisteredPerk { id, perk } in self.perks.iter() {
            let ctx = PerkContext {
                dick_id: &dick,
                intent: change_intent,
                source,
                state: states.of(*id),
                today: states.today,
                lang_code,
            };
            let PerkOutcome { change: AdditionalChange(ac), state, note, loan_payout } = perk.apply(ctx).await;
            if let Some(state) = state {
                writes.states.push(PerkStateUpdate { perk_id: *id, state });
            }
            if loan_payout.is_some() {
                writes.loan_payout = loan_payout;
            }
            if !ac.is_zero() {
                let change = SignedLengthChange::new(ac.value());
                by_perks.insert(perk.name(), PerkEffect { change, note });
            }
            // saturating addition: a perk pushing the sum out of i64 bounds clamps it
            // instead of wrapping; the checked addition below still decides the outcome
            additional_change += ac.value()
        }

        let total = (base + additional_change)
            .inspect_err(|e| tracing::error!(dick = %dick, error = %e, "an overflow in the increment calculation"))
            .unwrap_or(base);

        if base == total && !additional_change.is_zero() {
            tracing::info!(perks = ?by_perks, "some perks affected the calculation");
            by_perks.clear();
        }

        Increment { base, by_perks, total, writes }
    }
}

impl Increment {
    fn of_base(base: LengthChange) -> Self {
        Self {
            base,
            by_perks: HashMap::default(),
            total: base,
            writes: PerkWrites::default(),
        }
    }

    pub fn perks_part_of_answer(&self, lang_code: &str) -> String {
        if self.base.value() != self.total.value() {
            let top_line = t!("titles.perks.top_line", locale = lang_code);
            let perks = self.by_perks.iter()
                .map(|(perk, effect)| {
                    let t_key = format!("titles.perks.{perk}.name");
                    let name = t!(&t_key, locale = lang_code);
                    let value = effect.change;
                    match &effect.note {
                        Some(note) => format!("— {name} ({value:+}, {note})"),
                        None => format!("— {name} ({value:+})"),
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");
            format!("\n\n{top_line}:\n{perks}")
        } else {
            String::default()
        }
    }
}

fn get_base_increment<T>(range: RangeInclusive<T>, sign_ratio: Ratio) -> T
where
    T: PrimInt + PartialOrd + SampleUniform + From<i8>
{
    let percent: u32 = (sign_ratio.value() * 100.0).round().saturating_into();
    let sign_ratio_percent = match percent {
        ..=0 => 0,
        100.. => 100,
        x => x
    };
    let mut rng = rand::rng();
    let zero = <T as From<i8>>::from(0);
    if range.start() > &zero {
        return rng.random_range(range)
    }
    let positive = rng.random_ratio(sign_ratio_percent, 100);
    if positive {
        let end = *range.end();
        let one = <T as From<i8>>::from(1);
        rng.random_range(one..=end)
    } else {
        let start = *range.start();
        let minus_one = <T as From<i8>>::from(-1);
        rng.random_range(start..=minus_one)
    }
}

#[cfg(test)]
mod test {
    use domain_types::literal;
    use crate::domain::primitives::Ratio;
    use super::get_base_increment;

    #[test]
    fn test_gen_increment() {
        let increments: Vec<i32> = (0..100)
            .map(|_| get_base_increment(-5..=10, literal!(Ratio = 0.5)))
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
            .map(|_| get_base_increment(5..=10, literal!(Ratio = 0.5)))
            .collect();
        assert!(increments.iter().all(|n| n <= &10));
        assert!(increments.iter().all(|n| n >= &5));
    }
}

#[cfg(test)]
mod test_incrementor {
    use domain_types::literal;
    use std::iter::zip;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use futures::future::join_all;
    use crate::config::IncrementorConfig;
    use crate::domain::primitives::{DaysCount, LanguageCode, LengthChange, PerkName, Ratio};
    use crate::handlers::utils::{AdditionalChange, Incrementor, Perk, PerkContext, PerkOutcome};
    use crate::repo;
    use crate::repo::test::{CHAT_ID_KIND, fresh_db, USER_ID};

    #[tokio::test]
    async fn test_incrementor() {
        let db = fresh_db().await;
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let incr = Incrementor {
            config: IncrementorConfig {
                growth_range: -1..=1,
                grow_shrink_ratio: literal!(Ratio = 0.5),
                newcomers_grace_days: DaysCount::new(1),
                dod_bonus_range: 1..=2,
                perks: Default::default(),
            },
            dicks,
            perk_states: repo::PerkStates::new(db),
            perks: Vec::default()
        };

        test_growth_increment_base(&incr).await;
        test_dod_increment_base(&incr).await;
        test_with_perks(&incr).await;
        test_perks_cannot_cancel_growth(&incr).await;
        test_perks_cannot_cancel_dod(&incr).await;
        test_perk_with_overflow(&incr).await;
    }

    fn lang() -> LanguageCode {
        LanguageCode::new("en".to_owned())
    }

    async fn test_growth_increment_base(incr: &Incrementor) {
        let lang_code = lang();
        let lazy_vals = (0..100)
            .map(|_| incr.growth_increment(USER_ID, CHAT_ID_KIND, DaysCount::new(1), &lang_code));
        for fut in lazy_vals {
            let val = fut.await;
            assert_eq!(val.base, val.total);
            assert_ne!(val.base.value(), 0);
            assert!(val.base.value() >= -1);
            assert!(val.base.value() <= 1);
        }

        let lazy_positive_vals = (0..100)
            .map(|_| incr.growth_increment(USER_ID, CHAT_ID_KIND, DaysCount::new(0), &lang_code));
        for fut in lazy_positive_vals {
            let val = fut.await;
            assert_eq!(val.base, val.total);
            assert!(val.base.value() > 0);
        }
    }

    async fn test_dod_increment_base(incr: &Incrementor) {
        let lang_code = lang();
        let val = (0..100)
            .map(|_| incr.dod_increment(USER_ID, CHAT_ID_KIND, &lang_code));
        let val = join_all(val).await;
        assert!(val.iter().all(|n| { n.base == n.total }));
        assert!(val.iter().all(|n| { n.base.value() == 1 || n.base.value() == 2 }))
    }

    /// What a test perk has to say; the rest of `Perk` is the same for all of them.
    trait TestPerk: Send + Sync + 'static {
        fn label(&self) -> String;
        fn change(&self, ctx: &PerkContext<'_>) -> LengthChange;
    }

    #[async_trait]
    impl<T: TestPerk> Perk for T {
        fn name(&self) -> PerkName {
            PerkName::of(self.label()).expect("a valid perk name")
        }

        async fn apply(&self, ctx: PerkContext<'_>) -> PerkOutcome {
            AdditionalChange(self.change(&ctx)).into()
        }

        fn enabled(&self) -> bool {
            true
        }
    }

    #[derive(Clone)]
    struct AddPerk {
        value: i64,
    }

    impl AddPerk {
        fn boxed(value: i64) -> Box<Self> {
            Box::new(Self { value })
        }
    }

    impl TestPerk for AddPerk {
        fn label(&self) -> String {
            format!("add-perk-{}", self.value)
        }

        fn change(&self, _: &PerkContext<'_>) -> LengthChange {
            LengthChange::signed(self.value)
        }
    }

    /// Cancels the base increment out completely on its first `times` calls and does nothing after.
    #[derive(Clone)]
    struct CancelFirstPerk {
        times: usize,
        calls: Arc<AtomicUsize>,
    }

    impl CancelFirstPerk {
        fn boxed(times: usize) -> Box<Self> {
            Box::new(Self { times, calls: Arc::new(AtomicUsize::new(0)) })
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl TestPerk for CancelFirstPerk {
        fn label(&self) -> String {
            format!("cancel-first-{}", self.times)
        }

        fn change(&self, ctx: &PerkContext<'_>) -> LengthChange {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.times {
                LengthChange::signed(-ctx.intent.base_increment.value())
            } else {
                LengthChange::signed(0)
            }
        }
    }

    async fn test_with_perks(incr: &Incrementor) {
        let mut incr = incr.clone();
        let perk_plus2 = AddPerk::boxed(2);
        let perk_minus1 = AddPerk::boxed(-1);
        incr.set_perks(vec![perk_plus2.clone(), perk_minus1.clone()]);
        let lang_code = lang();

        let growth_lazy_vals = (0..100)
            .map(|_| incr.growth_increment(USER_ID, CHAT_ID_KIND, DaysCount::new(1), &lang_code));
        let dod_lazy_vals = (0..100)
            .map(|_| incr.dod_increment(USER_ID, CHAT_ID_KIND, &lang_code));

        macro_rules! assertions {
            ($val:ident) => {
                assert_eq!($val.total.value() - $val.base.value(), 1);
                assert_eq!($val.by_perks[&perk_plus2.name()].change, 2);
                assert_eq!($val.by_perks[&perk_minus1.name()].change, -1);
            };
        }

        for (growth_fut, dod_fut) in zip(growth_lazy_vals, dod_lazy_vals) {
            let (growth_val, dod_val) = (growth_fut.await, dod_fut.await);
            assertions!(growth_val);
            assertions!(dod_val);
        }
    }
    
    async fn test_perk_with_overflow(incr: &Incrementor) {
        let mut incr = incr.clone();
        // lengths are i64 now, so only an i64 overflow triggers the fallback
        let perk_add_max_int = AddPerk::boxed(i64::MAX);
        incr.set_perks(vec![perk_add_max_int.clone()]);

        let increment = incr.dod_increment(USER_ID, CHAT_ID_KIND, &lang()).await;
        assert_eq!(increment.base, increment.total);
        assert!(increment.by_perks.is_empty());
    }

    async fn test_perks_cannot_cancel_growth(incr: &Incrementor) {
        let mut incr = incr.clone();
        incr.config.growth_range = 1..=1;
        incr.set_perks(vec![AddPerk::boxed(-1)]);
        let positive = incr.growth_increment(USER_ID, CHAT_ID_KIND, DaysCount::new(0), &lang()).await;
        assert_eq!(positive.base.value(), 1);
        assert_eq!(positive.total.value(), 1);

        incr.config.growth_range = -1..=-1;
        incr.config.grow_shrink_ratio = literal!(Ratio = 0.0);
        incr.set_perks(vec![AddPerk::boxed(1)]);
        let negative = incr.growth_increment(USER_ID, CHAT_ID_KIND, DaysCount::new(2), &lang()).await;
        assert_eq!(negative.base.value(), -1);
        assert_eq!(negative.total.value(), -1);

        incr.config.growth_range = 1..=1;
        let perk = CancelFirstPerk::boxed(3);
        incr.set_perks(vec![perk.clone()]);
        let rerolled = incr.growth_increment(USER_ID, CHAT_ID_KIND, DaysCount::new(0), &lang()).await;
        assert_eq!(rerolled.total.value(), 1);
        assert_eq!(perk.calls(), 4);
    }

    async fn test_perks_cannot_cancel_dod(incr: &Incrementor) {
        let mut incr = incr.clone();
        incr.config.dod_bonus_range = 1..=1;
        incr.set_perks(vec![AddPerk::boxed(-1)]);
        let cancelled = incr.dod_increment(USER_ID, CHAT_ID_KIND, &lang()).await;
        assert_eq!(cancelled.base.value(), 1);
        assert_eq!(cancelled.total.value(), 1);

        let perk = CancelFirstPerk::boxed(3);
        incr.set_perks(vec![perk.clone()]);
        let rerolled = incr.dod_increment(USER_ID, CHAT_ID_KIND, &lang()).await;
        assert_eq!(rerolled.total.value(), 1);
        assert_eq!(perk.calls(), 4);
    }
}
