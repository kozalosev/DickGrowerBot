use std::collections::HashMap;
use std::ops::RangeInclusive;
use domain_types::traits::SaturatingInto;
use std::sync::Arc;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use chrono::NaiveDate;
use derive_more::Display;
use downcast_rs::{Downcast, impl_downcast};
use num_traits::PrimInt;
use rand::distr::uniform::SampleUniform;
use rand::RngExt;
use rust_i18n::t;
use sqlx::types::JsonValue;
use sqlx::PgConnection;
use crate::repo::{self, Dicks, Growth, PerkStates};
use crate::config::IncrementorConfig;
use crate::domain::objects::PerkStateUpdate;
use crate::domain::primitives::chat::{ChatIdKind, InternalChatId};
use crate::domain::primitives::{DaysCount, LanguageCode, Length, LengthChange, PerkId, PerkName, PerkNote, Ratio, SignedLengthChange, UserId};
use domain_types::literal;

const MAX_REROLLS: usize = 10;

#[derive(Clone)]
pub struct Incrementor {
    config: IncrementorConfig,
    perks: Vec<RegisteredPerk>,
    perk_states: repo::PerkStates,
}

#[async_trait]
pub trait Perk: Send + Sync + Downcast {
    fn name(&self) -> PerkName;
    async fn apply(&self, ctx: PerkContext<'_>) -> anyhow::Result<PerkOutcome>;

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
///
/// `conn` is the transaction of the growth. Whatever a perk writes through it is kept only if the
/// attempt is, and undone together with the rest when the base is rolled again.
pub struct PerkContext<'a> {
    pub dick_id: &'a DickId,
    pub chat_id: InternalChatId,
    pub intent: ChangeIntent,
    pub source: ChangeSource,
    pub state: Option<&'a JsonValue>,
    pub today: NaiveDate,
    pub lang_code: &'a LanguageCode,
    pub conn: &'a mut PgConnection,
}

/// What a perk made of it. The state is stored by the caller, in the same attempt as everything
/// else the perk wrote.
pub struct PerkOutcome {
    pub change: AdditionalChange,
    pub state: Option<JsonValue>,
    /// Extra detail for the "perks affected the result" line, e.g. how long a streak has grown to.
    /// Shown only next to a non-zero change.
    pub note: Option<PerkNote>,
}

impl From<AdditionalChange> for PerkOutcome {
    fn from(change: AdditionalChange) -> Self {
        Self { change, state: None, note: None }
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

    /// The growth of the dick, which is the transaction `growth` holds. The caller commits it with
    /// the total, or drops it to undo everything the perks wrote.
    pub async fn growth_increment(
        &self,
        growth: &mut Growth,
        days_since_registration: DaysCount,
        lang_code: &LanguageCode,
    ) -> Increment {
        let grow_shrink_ratio = if days_since_registration > self.config.newcomers_grace_days {
            self.config.grow_shrink_ratio
        } else {
            literal!(Ratio = 1.0)
        };
        self.nonzero_increment(growth, ChangeSource::Growth, lang_code, || {
            let base_incr = get_base_increment(self.config.growth_range.clone(), grow_shrink_ratio);
            SignedLengthChange::new(base_incr.into())
        }).await
    }

    pub async fn dod_increment(&self, growth: &mut Growth, lang_code: &LanguageCode) -> Increment {
        self.nonzero_increment(growth, ChangeSource::DickOfDay, lang_code, || {
            let base_incr = rand::rng().random_range(self.config.dod_bonus_range.clone());
            SignedLengthChange::new(base_incr.into())
        }).await
    }

    /// Rolls the base again for as long as the perks cancel it out. Any failure along the way
    /// ends the rerolls and the perks are skipped.
    async fn nonzero_increment(
        &self,
        growth: &mut Growth,
        source: ChangeSource,
        lang_code: &LanguageCode,
        roll_base: impl Fn() -> BaseIncrement,
    ) -> Increment {
        let dick = DickId(growth.key().uid, growth.key().chat_kind.clone());
        for _ in 0..MAX_REROLLS {
            match self.try_base(growth, &dick, roll_base(), source, lang_code).await {
                Ok(Some(increment)) => return increment,
                Ok(None) => tracing::debug!(%dick, "perks cancelled the increment, rerolling the base"),
                Err(e) => {
                    tracing::error!(%dick, error = format!("{e:#}"), "couldn't apply the perks");
                    break
                },
            }
        }
        tracing::warn!(%dick, "the perks were skipped");
        roll_base().only()
    }

    /// One attempt is a savepoint: it is released when the perks leave something of the roll, and
    /// rolled back with whatever the perks wrote when they leave nothing or fail.
    async fn try_base(
        &self,
        growth: &mut Growth,
        dick: &DickId,
        base: BaseIncrement,
        source: ChangeSource,
        lang_code: &LanguageCode,
    ) -> anyhow::Result<Option<Increment>> {
        let chat_id = growth.key().chat_id;
        let mut attempt = growth.attempt().await?;
        let applied = self.add_additional_incr(&mut attempt, dick, chat_id, base, source, lang_code).await;
        match applied {
            Ok(increment) if !increment.total.is_zero() => {
                attempt.commit().await?;
                Ok(Some(increment))
            },
            Ok(_) => {
                attempt.rollback().await?;
                Ok(None)
            },
            Err(e) => {
                let _ = attempt.rollback().await;
                Err(e)
            },
        }
    }

    async fn add_additional_incr(
        &self,
        conn: &mut PgConnection,
        dick: &DickId,
        chat_id: InternalChatId,
        base_increment: BaseIncrement,
        source: ChangeSource,
        lang_code: &LanguageCode,
    ) -> anyhow::Result<Increment> {
        let current_length = Dicks::fetch_length_internal(conn, dick.0, chat_id).await?;
        let states = PerkStates::read_internal(conn, dick.0, chat_id).await?;
        let base = LengthChange::from(base_increment);
        let change_intent = ChangeIntent {
            base_increment: base,
            current_length,
        };

        let mut additional_change = SignedLengthChange::new(0);
        let mut by_perks = HashMap::new();
        let mut new_states = Vec::new();
        for RegisteredPerk { id, perk } in self.perks.iter() {
            let ctx = PerkContext {
                dick_id: dick,
                chat_id,
                intent: change_intent,
                source,
                state: states.of(*id),
                today: states.today,
                lang_code,
                conn: &mut *conn,
            };
            let PerkOutcome { change: AdditionalChange(ac), state, note } = perk.apply(ctx).await?;
            if let Some(state) = state {
                new_states.push(PerkStateUpdate { perk_id: *id, state });
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
            .context("an overflow in the increment calculation")?;
        PerkStates::write_all_internal(conn, chat_id, dick.0, &new_states).await?;
        Ok(Increment { base, by_perks, total })
    }
}

impl Increment {
    fn of_base(base: LengthChange) -> Self {
        Self {
            base,
            by_perks: HashMap::default(),
            total: base,
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use sqlx::{Pool, Postgres};
    use crate::config::IncrementorConfig;
    use crate::domain::objects::DickOfDayResult;
    use crate::domain::primitives::{Debt, DaysCount, LanguageCode, LengthChange, PayoutRatio, PerkName, Ratio};
    use crate::handlers::perks::LoanPayoutPerk;
    use crate::handlers::utils::{AdditionalChange, Incrementor, Perk, PerkContext, PerkOutcome};
    use crate::repo;
    use crate::repo::Growth;
    use crate::repo::test::dicks::{create_dick, create_user};
    use crate::repo::test::{CHAT_ID_KIND, fresh_db, UID, CHAT_ID, USER_ID};

    #[tokio::test]
    async fn test_incrementor() {
        let db = fresh_db().await;
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let incr = incrementor(&db, IncrementorConfig {
            growth_range: -1..=1,
            grow_shrink_ratio: literal!(Ratio = 0.5),
            newcomers_grace_days: DaysCount::new(1),
            dod_bonus_range: 1..=2,
            perks: Default::default(),
        });

        test_growth_increment_base(&incr, &dicks).await;
        test_dod_increment_base(&incr, &dicks).await;
        test_with_perks(&incr, &dicks).await;
        test_perks_cannot_cancel_growth(&incr, &dicks).await;
        test_perks_cannot_cancel_dod(&incr, &dicks).await;
        test_perk_with_overflow(&incr, &dicks).await;
        test_failing_perk(&incr, &dicks).await;
    }

    #[tokio::test]
    async fn a_reroll_takes_back_what_the_perks_wrote() {
        let db = fresh_db().await;
        create_user(&db).await;
        create_dick(&db).await;
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let config = IncrementorConfig {
            growth_range: 3..=3,
            grow_shrink_ratio: literal!(Ratio = 1.0),
            newcomers_grace_days: DaysCount::new(0),
            dod_bonus_range: 1..=1,
            perks: Default::default(),
        };

        // the payout cancels the whole roll out, so every attempt pays and is rolled back
        let loans = take_loan(&db, literal!(PayoutRatio = 0.9)).await;
        let mut incr = incrementor(&db, config.clone());
        incr.set_perks(vec![Box::new(LoanPayoutPerk)]);
        sqlx::query("UPDATE Dicks SET updated_at = updated_at - interval '1 day'")
            .execute(&db)
            .await.expect("couldn't move the last growth to yesterday");
        let mut growth = begin(&dicks).await;
        let increment = incr.growth_increment(&mut growth, DaysCount::new(10), &lang()).await;
        growth.commit_growth(increment.total)
            .await.expect("couldn't commit the growth");
        assert_eq!(increment.total.value(), 3);
        assert_eq!(debt_of(&loans).await, 10);

        // half of the roll is paid, so the first attempt is kept and pays once
        let loans = take_loan(&db, literal!(PayoutRatio = 0.5)).await;
        let config = IncrementorConfig { growth_range: 4..=4, ..config };
        let mut incr = incrementor(&db, config);
        incr.set_perks(vec![Box::new(LoanPayoutPerk)]);
        sqlx::query("UPDATE Dicks SET updated_at = updated_at - interval '1 day'")
            .execute(&db)
            .await.expect("couldn't move the last growth to yesterday again");
        let mut growth = begin(&dicks).await;
        let increment = incr.growth_increment(&mut growth, DaysCount::new(10), &lang()).await;
        growth.commit_growth(increment.total)
            .await.expect("couldn't commit the growth");
        assert_eq!(increment.total.value(), 2);
        assert_eq!(debt_of(&loans).await, 8);
    }

    /// A growth locks the dick and then the loan, and `Loans::borrow` does the same, so the two can
    /// run at once without waiting for each other and without losing either change.
    #[tokio::test]
    async fn a_growth_and_a_borrowing_do_not_deadlock() {
        let db = fresh_db().await;
        create_user(&db).await;
        create_dick(&db).await;
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let loans = take_loan(&db, literal!(PayoutRatio = 0.5)).await;
        let mut incr = incrementor(&db, IncrementorConfig {
            growth_range: 4..=4,
            grow_shrink_ratio: literal!(Ratio = 1.0),
            newcomers_grace_days: DaysCount::new(0),
            dod_bonus_range: 1..=1,
            perks: Default::default(),
        });
        incr.set_perks(vec![Box::new(LoanPayoutPerk)]);
        // borrowing has just brought the length back to zero, and a loan needs a negative one
        sqlx::query("UPDATE Dicks SET length = -10, updated_at = updated_at - interval '1 day'")
            .execute(&db)
            .await.expect("couldn't move the last growth to yesterday");

        // the growth holds the dick and the loan, and the borrowing has to queue behind it
        let mut growth = begin(&dicks).await;
        let increment = incr.growth_increment(&mut growth, DaysCount::new(10), &lang()).await;
        let borrowing = tokio::spawn({
            let loans = loans.clone();
            async move { loans.borrow(USER_ID, &CHAT_ID_KIND, Debt::new(5)).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!borrowing.is_finished(), "the borrowing must wait for the growth that holds the dick");

        let grown = growth.commit_growth(increment.total)
            .await.expect("couldn't commit the growth");
        let borrowed = tokio::time::timeout(std::time::Duration::from_secs(10), borrowing)
            .await.expect("the borrowing must go on once the growth is committed")
            .expect("the borrowing must not panic")
            .expect("couldn't refinance the loan");

        assert!(grown.is_some());
        assert_eq!(borrowed, repo::BorrowResult::Granted);
        // ten owed, two paid out of the growth, five more borrowed
        assert_eq!(debt_of(&loans).await, 13);
    }

    /// Replaces the loan of the dick with a new one of ten.
    async fn take_loan(db: &Pool<Postgres>, payout_ratio: PayoutRatio) -> repo::Loans {
        sqlx::query("DELETE FROM Loans")
            .execute(db)
            .await.expect("couldn't remove the old loans");
        sqlx::query("UPDATE Dicks SET length = -10 WHERE uid = $1 AND chat_id = (SELECT id FROM Chats WHERE chat_id = $2)")
            .bind(UID)
            .bind(CHAT_ID)
            .execute(db)
            .await.expect("couldn't set the length");
        let loans = repo::Loans::new(db.clone(), &crate::config::AppConfig {
            loan_payout_ratio: payout_ratio,
            ..Default::default()
        });
        loans.borrow(USER_ID, &CHAT_ID_KIND, Debt::new(10))
            .await.expect("couldn't apply for a loan");
        loans
    }

    async fn debt_of(loans: &repo::Loans) -> Debt {
        loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the loan")
            .expect("the loan must be present")
            .debt
    }

    fn incrementor(db: &Pool<Postgres>, config: IncrementorConfig) -> Incrementor {
        Incrementor {
            config,
            perk_states: repo::PerkStates::new(db.clone()),
            perks: Vec::default()
        }
    }

    async fn begin(dicks: &repo::Dicks) -> Growth {
        dicks.begin_growth(USER_ID, &CHAT_ID_KIND.into())
            .await.expect("couldn't begin a growth")
    }

    fn lang() -> LanguageCode {
        LanguageCode::new("en".to_owned())
    }

    async fn test_growth_increment_base(incr: &Incrementor, dicks: &repo::Dicks) {
        let lang_code = lang();
        for _ in 0..100 {
            let mut growth = begin(dicks).await;
            let val = incr.growth_increment(&mut growth, DaysCount::new(1), &lang_code).await;
            assert_eq!(val.base, val.total);
            assert_ne!(val.base.value(), 0);
            assert!(val.base.value() >= -1);
            assert!(val.base.value() <= 1);
        }

        for _ in 0..100 {
            let mut growth = begin(dicks).await;
            let val = incr.growth_increment(&mut growth, DaysCount::new(0), &lang_code).await;
            assert_eq!(val.base, val.total);
            assert!(val.base.value() > 0);
        }
    }

    async fn test_dod_increment_base(incr: &Incrementor, dicks: &repo::Dicks) {
        let lang_code = lang();
        for _ in 0..100 {
            let mut growth = begin(dicks).await;
            let val = incr.dod_increment(&mut growth, &lang_code).await;
            assert_eq!(val.base, val.total);
            assert!(val.base.value() == 1 || val.base.value() == 2);
        }
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

        async fn apply(&self, ctx: PerkContext<'_>) -> anyhow::Result<PerkOutcome> {
            Ok(AdditionalChange(self.change(&ctx)).into())
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

    async fn test_with_perks(incr: &Incrementor, dicks: &repo::Dicks) {
        let mut incr = incr.clone();
        let perk_plus2 = AddPerk::boxed(2);
        let perk_minus1 = AddPerk::boxed(-1);
        incr.set_perks(vec![perk_plus2.clone(), perk_minus1.clone()]);
        let lang_code = lang();

        macro_rules! assertions {
            ($val:ident) => {
                assert_eq!($val.total.value() - $val.base.value(), 1);
                assert_eq!($val.by_perks[&perk_plus2.name()].change, 2);
                assert_eq!($val.by_perks[&perk_minus1.name()].change, -1);
            };
        }

        for _ in 0..100 {
            let mut growth = begin(dicks).await;
            let growth_val = incr.growth_increment(&mut growth, DaysCount::new(1), &lang_code).await;
            assertions!(growth_val);
            let mut growth = begin(dicks).await;
            let dod_val = incr.dod_increment(&mut growth, &lang_code).await;
            assertions!(dod_val);
        }
    }

    /// A perk whose query fails, which poisons a Postgres transaction unless it is rolled back.
    struct FailingPerk;

    #[async_trait]
    impl Perk for FailingPerk {
        fn name(&self) -> PerkName {
            literal!(PerkName = "failing")
        }

        async fn apply(&self, ctx: PerkContext<'_>) -> anyhow::Result<PerkOutcome> {
            sqlx::query("SELECT * FROM there_is_no_such_table")
                .execute(ctx.conn)
                .await?;
            Ok(AdditionalChange::zero().into())
        }

        fn enabled(&self) -> bool {
            true
        }
    }

    async fn test_failing_perk(incr: &Incrementor, dicks: &repo::Dicks) {
        let mut incr = incr.clone();
        incr.set_perks(vec![AddPerk::boxed(2), Box::new(FailingPerk)]);

        let mut growth = begin(dicks).await;
        let increment = incr.dod_increment(&mut growth, &lang()).await;
        assert_eq!(increment.base, increment.total);
        assert!(increment.by_perks.is_empty());

        let result = growth.commit_dod(increment.total)
            .await.expect("the growth must still be usable after a perk failed");
        assert!(matches!(result, DickOfDayResult::NoDick | DickOfDayResult::Chosen(_)));
    }

    async fn test_perk_with_overflow(incr: &Incrementor, dicks: &repo::Dicks) {
        let mut incr = incr.clone();
        // lengths are i64 now, so only an i64 overflow triggers the fallback
        let perk_add_max_int = AddPerk::boxed(i64::MAX);
        incr.set_perks(vec![perk_add_max_int.clone()]);

        let mut growth = begin(dicks).await;
        let increment = incr.dod_increment(&mut growth, &lang()).await;
        assert_eq!(increment.base, increment.total);
        assert!(increment.by_perks.is_empty());
    }

    async fn test_perks_cannot_cancel_growth(incr: &Incrementor, dicks: &repo::Dicks) {
        let mut incr = incr.clone();
        incr.config.growth_range = 1..=1;
        incr.set_perks(vec![AddPerk::boxed(-1)]);
        let mut growth = begin(dicks).await;
        let positive = incr.growth_increment(&mut growth, DaysCount::new(0), &lang()).await;
        assert_eq!(positive.base.value(), 1);
        assert_eq!(positive.total.value(), 1);

        incr.config.growth_range = -1..=-1;
        incr.config.grow_shrink_ratio = literal!(Ratio = 0.0);
        incr.set_perks(vec![AddPerk::boxed(1)]);
        let mut growth = begin(dicks).await;
        let negative = incr.growth_increment(&mut growth, DaysCount::new(2), &lang()).await;
        assert_eq!(negative.base.value(), -1);
        assert_eq!(negative.total.value(), -1);

        incr.config.growth_range = 1..=1;
        let perk = CancelFirstPerk::boxed(3);
        incr.set_perks(vec![perk.clone()]);
        let mut growth = begin(dicks).await;
        let rerolled = incr.growth_increment(&mut growth, DaysCount::new(0), &lang()).await;
        assert_eq!(rerolled.total.value(), 1);
        assert_eq!(perk.calls(), 4);
    }

    async fn test_perks_cannot_cancel_dod(incr: &Incrementor, dicks: &repo::Dicks) {
        let mut incr = incr.clone();
        incr.config.dod_bonus_range = 1..=1;
        incr.set_perks(vec![AddPerk::boxed(-1)]);
        let mut growth = begin(dicks).await;
        let cancelled = incr.dod_increment(&mut growth, &lang()).await;
        assert_eq!(cancelled.base.value(), 1);
        assert_eq!(cancelled.total.value(), 1);

        let perk = CancelFirstPerk::boxed(3);
        incr.set_perks(vec![perk.clone()]);
        let mut growth = begin(dicks).await;
        let rerolled = incr.dod_increment(&mut growth, &lang()).await;
        assert_eq!(rerolled.total.value(), 1);
        assert_eq!(perk.calls(), 4);
    }
}
