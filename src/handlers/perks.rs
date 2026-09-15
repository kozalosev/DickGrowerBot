use async_trait::async_trait;
use chrono::NaiveDate;
use domain_types::traits::{ApproxInto, SaturatingInto};
use crate::domain::primitives::Coefficient;
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use sqlx::types::JsonValue;
use sqlx::{Pool, Postgres};
use crate::handlers::utils::{days_word_ru, AdditionalChange, ChangeSource, ConfigurablePerk, Perk, PerkContext, PerkOutcome};
use crate::config::StreakGrades;
use crate::{config, repo};
use crate::domain::primitives::{DaysCount, LanguageCode, Length, LengthChange, LoanPayout, PerkName, PerkNote, Ratio};
use domain_types::literal;

const HELP_PUSSIES: &str = "help-pussies";
const LOAN_PAYOUT: &str = "loan-payout";
const STREAK: &str = "streak";

pub fn all(pool: &Pool<Postgres>, cfg: &config::AppConfig) -> Vec<Box<dyn Perk>> {
    let loans = repo::Loans::new(pool.clone(), cfg);
    let perks = &cfg.incrementor.perks;

    vec![
        Box::new(HelpPussiesPerk {
            coefficient: perks.help_pussies_ratio,
        }),
        Box::new(LoanPayoutPerk { loans }),
        Box::new(StreakPerk { grades: perks.streak_grades.clone() })
    ]
}

pub struct HelpPussiesPerk {
    coefficient: Ratio
}

#[async_trait]
impl Perk for HelpPussiesPerk {
    fn name(&self) -> PerkName {
        literal!(PerkName = HELP_PUSSIES)
    }

    async fn apply(&self, ctx: PerkContext<'_>) -> PerkOutcome {
        if ctx.intent.current_length >= Length::new(0) {
            return AdditionalChange::zero().into()
        }

        let current_deepness: f64 = ctx.intent.current_length.abs().approx_into();
        let change: i64 = self.coefficient.scale(current_deepness).round().saturating_into();
        AdditionalChange(LengthChange::signed(change)).into()
    }

    fn enabled(&self) -> bool {
        self.coefficient > literal!(Ratio = 0.0)
    }
}

impl ConfigurablePerk for HelpPussiesPerk {
    type Config = Ratio;

    fn get_config(&self) -> Self::Config {
        self.coefficient
    }
}

pub struct LoanPayoutPerk {
    loans: repo::Loans,
}

#[async_trait]
impl Perk for LoanPayoutPerk {
    fn name(&self) -> PerkName {
        literal!(PerkName = LOAN_PAYOUT)
    }

    async fn apply(&self, ctx: PerkContext<'_>) -> PerkOutcome {
        let dick_id = ctx.dick_id;
        let maybe_loan_components = self.loans.get_active_loan(dick_id.0, &dick_id.1)
            .await
            .inspect_err(|e| tracing::error!(error = %e, "couldn't check whether a perk is active"))
            .ok()
            .flatten()
            .map(|loan| (loan.debt, loan.payout_ratio));
        let (debt, payout_coefficient) = match maybe_loan_components {
            Some(x) => x,
            None => return AdditionalChange::zero().into()
        };

        let base_increment = ctx.intent.base_increment.value();
        let payout_value = if base_increment.is_positive() {
            // the coefficient is a Ratio [0; 1], so the payout never exceeds the base increment
            let payout: i64 = payout_coefficient.scale(base_increment.approx_into()).round().saturating_into();
            payout.min(debt.saturating_into())
        } else {
            0
        };
        let payout = u32::try_from(payout_value)
            .map(LoanPayout::new)
            .unwrap_or_else(|e| {
                tracing::error!(payout = payout_value, dick_id = %dick_id, error = %e, "the loan payout is invalid");
                LoanPayout::new(0)
            });
        match self.loans.pay(dick_id.0, &dick_id.1, payout).await {
            Ok(()) => AdditionalChange(LengthChange::signed(-i64::from(payout.value()))).into(),
            Err(e) => {
                tracing::error!(payout = %payout, dick_id = %dick_id, error = %e, "couldn't pay for the loan");
                AdditionalChange::zero().into()
            }
        }
    }
}

/// Adds a centimetre to a growth for every grade its owner's days in a row have reached. A shrink
/// gets the same centimetres, which soften it.
pub struct StreakPerk {
    grades: StreakGrades,
}

/// What a streak needs to remember. `last_grow` is a date rather than a count of days, because the
/// count would be wrong the moment the bot spent a day not running.
#[derive(Serialize, Deserialize)]
struct StreakState {
    streak: DaysCount,
    max: DaysCount,
    last_grow: NaiveDate,
}

#[async_trait]
impl Perk for StreakPerk {
    fn name(&self) -> PerkName {
        literal!(PerkName = STREAK)
    }

    async fn apply(&self, ctx: PerkContext<'_>) -> PerkOutcome {
        if ctx.source != ChangeSource::Growth {
            return AdditionalChange::zero().into()
        }

        let previous = ctx.state.and_then(parse_streak);
        let streak = match &previous {
            // the same day again means an extra attempt was spent, not another day of playing
            Some(prev) if prev.last_grow == ctx.today => prev.streak,
            Some(prev) if prev.last_grow.succ_opt() == Some(ctx.today) => prev.streak + 1,
            _ => DaysCount::new(1),
        };
        let max = previous.map_or(streak, |prev| prev.max.max(streak));
        let state = StreakState { streak, max, last_grow: ctx.today };

        let bonus: i64 = self.grades.reached(streak).saturating_into();
        let days = t!("titles.perks.streak.note", locale = ctx.lang_code,
            days = streak, word_days = days_word_ru(streak));
        let note = self.grades.next(streak)
            .map(|next_day| t!("titles.perks.streak.next_grade", locale = ctx.lang_code,
                next_bonus = bonus + 1, next_day = next_day))
            .map_or_else(|| days.to_string(), |next_grade| format!("{days}; {next_grade}"));

        PerkOutcome {
            change: AdditionalChange(LengthChange::signed(bonus)),
            state: serde_json::to_value(state)
                .inspect_err(|e| tracing::error!(dick_id = %ctx.dick_id, error = %e, "couldn't serialize a streak"))
                .ok(),
            note: Some(PerkNote::new(note)),
        }
    }

    fn stats_line(&self, state: Option<&JsonValue>, lang_code: &LanguageCode) -> Option<String> {
        let state = state.and_then(parse_streak)?;
        Some(t!("commands.stats.streak", locale = lang_code,
            current = state.streak, max = state.max).to_string())
    }

    fn enabled(&self) -> bool {
        !self.grades.is_empty()
    }
}

impl ConfigurablePerk for StreakPerk {
    type Config = StreakGrades;

    fn get_config(&self) -> Self::Config {
        self.grades.clone()
    }
}

/// A blob that doesn't parse belongs to an older shape of this perk or to nobody; either way the
/// streak starts over rather than the growth failing.
fn parse_streak(state: &JsonValue) -> Option<StreakState> {
    serde_json::from_value(state.clone())
        .inspect_err(|e| tracing::warn!(error = %e, "couldn't read a stored streak"))
        .ok()
}

#[cfg(test)]
mod test {
    use chrono::NaiveDate;
    use domain_types::literal;
    use sqlx::types::JsonValue;
    use crate::handlers::perks::{HelpPussiesPerk, LoanPayoutPerk, StreakPerk, StreakState};
    use crate::handlers::utils::{ChangeIntent, ChangeSource, DickId, Perk, PerkContext};
    use crate::{config, repo};
    use std::sync::LazyLock;
    use crate::domain::primitives::{DaysCount, Debt, LanguageCode, Length, LengthChange, LengthIncrement, PayoutRatio, Ratio, SignedLengthChange};
    use crate::repo::test::{CHAT_ID_KIND, fresh_db, USER_ID};

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 17).expect("a valid date")
    }

    static LANG_CODE: LazyLock<LanguageCode> = LazyLock::new(|| LanguageCode::new("en".to_owned()));

    /// A growth with nothing remembered about it, which is what both of the stateless perks get.
    fn ctx(dick_id: &DickId, intent: ChangeIntent) -> PerkContext<'_> {
        PerkContext {
            dick_id, intent, source: ChangeSource::Growth, state: None, today: today(), lang_code: &LANG_CODE,
        }
    }

    #[tokio::test]
    async fn test_help_pussies() {
        {
            let invalid_perk = HelpPussiesPerk { coefficient: literal!(Ratio = 0.0) };
            assert!(!invalid_perk.enabled())
        }

        let perk = HelpPussiesPerk { coefficient: literal!(Ratio = 0.5) };
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let change_intent_positive_length = ChangeIntent { current_length: Length::new(1), base_increment: LengthIncrement::new(1).into() };
        let change_intent_negative_length_positive_increment = ChangeIntent { current_length: Length::new(-1), base_increment: LengthIncrement::new(1).into() };
        let change_intent_negative_length_negative_increment = ChangeIntent { current_length: Length::new(-1), base_increment: SignedLengthChange::new(-1).into() };

        assert!(perk.enabled());
        assert_eq!(perk.apply(ctx(&dick_id, change_intent_positive_length)).await.change.0.value(), 0);
        assert_eq!(perk.apply(ctx(&dick_id, change_intent_negative_length_positive_increment)).await.change.0.value(), 1);
        assert_eq!(perk.apply(ctx(&dick_id, change_intent_negative_length_negative_increment)).await.change.0.value(), 1);
    }

    #[tokio::test]
    async fn test_loan_payout() {
        let db = fresh_db().await;
        let loans = {
            let cfg = config::AppConfig {
                loan_payout_ratio: literal!(PayoutRatio = 0.1),
                ..Default::default()
            };
            repo::Loans::new(db.clone(), &cfg)
        };

        {
            let users = repo::Users::new(db.clone());
            users.create_or_update(USER_ID, "")
                .await.expect("couldn't create a user");

            let dicks = repo::Dicks::new(db, Default::default());
            // the length must be negative to be eligible for a loan
            dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(-10), &[])
                .await.expect("couldn't create a dick");
        }

        let perk = LoanPayoutPerk { loans: loans.clone() };
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let change_intent_positive_increment = ChangeIntent { current_length: Length::new(1), base_increment: LengthIncrement::new(10).into() };
        let change_intent_positive_increment_small = ChangeIntent { current_length: Length::new(1), base_increment: LengthIncrement::new(2).into() };
        let change_intent_negative_increment = ChangeIntent { current_length: Length::new(1), base_increment: SignedLengthChange::new(-1).into() };

        assert!(perk.enabled());
        assert_eq!(perk.apply(ctx(&dick_id, change_intent_positive_increment)).await.change.0.value(), 0);

        let borrow_result = loans.borrow(USER_ID, &CHAT_ID_KIND, Debt::new(10))
            .await.expect("couldn't create a loan");
        assert_eq!(borrow_result, repo::BorrowResult::Granted);

        assert_eq!(perk.apply(ctx(&dick_id, change_intent_positive_increment)).await.change.0.value(), -1);
        let debt = loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the active loan")
            .expect("loan must be found")
            .debt;
        assert_eq!(debt, Debt::new(9));

        assert_eq!(perk.apply(ctx(&dick_id, change_intent_positive_increment_small)).await.change.0.value(), 0);
        assert_eq!(perk.apply(ctx(&dick_id, change_intent_negative_increment)).await.change.0.value(), 0);
        let debt = loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the active loan")
            .expect("loan must be found")
            .debt;
        assert_eq!(debt, Debt::new(9));
    }

    /// Grades on the second, third and fifth days, so every case is a few days away.
    fn streak_perk() -> StreakPerk {
        StreakPerk {
            grades: "2,3,5".parse().expect("couldn't parse the grades"),
        }
    }

    fn stored(streak: u32, max: u32, last_grow: NaiveDate) -> JsonValue {
        let state = StreakState {
            streak: DaysCount::new(streak),
            max: DaysCount::new(max),
            last_grow,
        };
        serde_json::to_value(state).expect("a streak must serialize")
    }

    #[tokio::test]
    async fn streak_is_worth_nothing_on_the_first_day() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let intent = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };

        let outcome = perk.apply(ctx(&dick_id, intent)).await;
        assert_eq!(outcome.change.0.value(), 0);
        let state = outcome.state.expect("the first day must be remembered");
        assert_eq!(state, stored(1, 1, today()));
    }

    #[tokio::test]
    async fn every_grade_reached_adds_a_centimetre_whatever_the_roll() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let yesterday = today().pred_opt().expect("a valid date");
        let stored_streak = stored(2, 2, yesterday);

        let gain = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };
        let outcome = perk.apply(PerkContext { state: Some(&stored_streak), ..ctx(&dick_id, gain) }).await;
        // the third day in a row has reached the grades of the second and the third
        assert_eq!(outcome.change.0.value(), 2);
        assert_eq!(outcome.state.expect("the streak must be remembered"), stored(3, 3, today()));

        let loss = ChangeIntent { current_length: Length::new(0), base_increment: SignedLengthChange::new(-10).into() };
        let outcome = perk.apply(PerkContext { state: Some(&stored_streak), ..ctx(&dick_id, loss) }).await;
        assert_eq!(outcome.change.0.value(), 2, "a shrink must be softened by the same centimetres");
    }

    #[tokio::test]
    async fn the_note_names_the_next_grade_until_the_last_one() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let yesterday = today().pred_opt().expect("a valid date");
        let intent = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };

        let before_last = stored(2, 2, yesterday);
        let outcome = perk.apply(PerkContext { state: Some(&before_last), ..ctx(&dick_id, intent) }).await;
        let note = outcome.note.expect("a streak must be noted").to_string();
        assert_eq!(note, "3 days in a row; +3 from day 5");

        let after_last = stored(4, 4, yesterday);
        let outcome = perk.apply(PerkContext { state: Some(&after_last), ..ctx(&dick_id, intent) }).await;
        let note = outcome.note.expect("a streak must be noted").to_string();
        assert_eq!(note, "5 days in a row");
    }

    #[tokio::test]
    async fn a_long_streak_stops_growing_at_the_last_grade() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let yesterday = today().pred_opt().expect("a valid date");
        let stored_streak = stored(99, 99, yesterday);
        let intent = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };

        let outcome = perk.apply(PerkContext { state: Some(&stored_streak), ..ctx(&dick_id, intent) }).await;
        assert_eq!(outcome.change.0.value(), 3);
        assert_eq!(outcome.state.expect("the streak must be remembered"), stored(100, 100, today()));
    }

    #[tokio::test]
    async fn a_second_growth_the_same_day_keeps_the_streak_where_it_is() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let stored_streak = stored(3, 5, today());
        let intent = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };

        let outcome = perk.apply(PerkContext { state: Some(&stored_streak), ..ctx(&dick_id, intent) }).await;
        assert_eq!(outcome.change.0.value(), 2, "the extra attempt is paid the same bonus");
        assert_eq!(outcome.state.expect("the streak must be remembered"), stored(3, 5, today()));
    }

    #[tokio::test]
    async fn a_missed_day_starts_the_streak_over_but_keeps_the_record() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let long_ago = today().pred_opt().and_then(|d| d.pred_opt()).expect("a valid date");
        let stored_streak = stored(7, 7, long_ago);
        let intent = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };

        let outcome = perk.apply(PerkContext { state: Some(&stored_streak), ..ctx(&dick_id, intent) }).await;
        assert_eq!(outcome.change.0.value(), 0);
        assert_eq!(outcome.state.expect("the streak must be remembered"), stored(1, 7, today()));
    }

    #[tokio::test]
    async fn a_dick_of_the_day_award_is_not_a_day_of_playing() {
        let perk = streak_perk();
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let yesterday = today().pred_opt().expect("a valid date");
        let stored_streak = stored(2, 2, yesterday);
        let intent = ChangeIntent { current_length: Length::new(0), base_increment: LengthIncrement::new(10).into() };

        let outcome = perk.apply(PerkContext {
            state: Some(&stored_streak),
            source: ChangeSource::DickOfDay,
            ..ctx(&dick_id, intent)
        }).await;
        assert_eq!(outcome.change.0.value(), 0);
        assert!(outcome.state.is_none(), "an award must not advance the streak");
    }

    #[test]
    fn the_streak_is_off_without_grades() {
        let without_grades = StreakPerk {
            grades: "".parse().expect("couldn't parse the grades"),
        };
        assert!(!without_grades.enabled());

        assert!(streak_perk().enabled());
    }
}
