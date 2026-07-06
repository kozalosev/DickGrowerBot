use async_trait::async_trait;
use sqlx::{Pool, Postgres};
use crate::handlers::utils::{AdditionalChange, ChangeIntent, ConfigurablePerk, DickId, Perk};
use crate::{config, repo};
use crate::domain::primitives::{Length, LengthChange, LoanPayout, Ratio, SignedLengthChange};

pub fn all(pool: &Pool<Postgres>, cfg: &config::AppConfig) -> Vec<Box<dyn Perk>> {
    let help_pussies_coef = Ratio::new(config::get_env_value_or_default("HELP_PUSSIES_COEF", 0.0))
        .unwrap_or(Ratio::literal(0.0));
    let loans = repo::Loans::new(pool.clone(), cfg);
    
    vec![
        Box::new(HelpPussiesPerk {
            coefficient: help_pussies_coef,
        }),
        Box::new(LoanPayoutPerk { loans })
    ]
}

pub struct HelpPussiesPerk {
    coefficient: Ratio
}

#[async_trait]
impl Perk for HelpPussiesPerk {
    fn name(&self) -> &str {
        "help-pussies"
    }

    async fn apply(&self, _: &DickId, change_intent: ChangeIntent) -> AdditionalChange {
        if change_intent.current_length >= Length::new(0) {
            return AdditionalChange::zero()
        }

        let current_deepness = change_intent.current_length.abs() as f64;
        // the coefficient is a Ratio, but the scaled deepness is not: multiply raw values
        let change = (self.coefficient.value() * current_deepness).round() as i64;
        AdditionalChange(LengthChange::Signed(SignedLengthChange::new(change)))
    }

    fn enabled(&self) -> bool {
        self.coefficient > Ratio::literal(0.0)
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
    fn name(&self) -> &str {
        "loan-payout"
    }

    async fn apply(&self, dick_id: &DickId, change_intent: ChangeIntent) -> AdditionalChange {
        let maybe_loan_components = self.loans.get_active_loan(dick_id.0, &dick_id.1)
            .await
            .inspect_err(|e| log::error!("couldn't check if a perk is active: {e}"))
            .ok()
            .flatten()
            .map(|loan| (loan.debt, loan.payout_ratio));
        let (debt, payout_coefficient) = match maybe_loan_components {
            Some(x) => x,
            None => return AdditionalChange::zero()
        };

        let base_increment = change_intent.base_increment.value();
        let payout_value = if base_increment.is_positive() {
            // the coefficient is a Ratio [0; 1], so the payout never exceeds the base increment
            let payout = (base_increment as f64 * payout_coefficient.value()).round() as i64;
            payout.min(debt.value())
        } else {
            0
        };
        let payout = LoanPayout::new(payout_value.clamp(0, i32::MAX as i64) as i32)
            .expect("loan payout is non-negative by construction");
        match self.loans.pay(dick_id.0, &dick_id.1, payout).await {
            Ok(()) => AdditionalChange(LengthChange::Signed(SignedLengthChange::new(-i64::from(payout.value())))),
            Err(e) => {
                log::error!("couldn't pay {payout} cm for the loan ({dick_id}): {e}");
                AdditionalChange::zero()
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::handlers::perks::{HelpPussiesPerk, LoanPayoutPerk};
    use crate::handlers::utils::{ChangeIntent, DickId, Perk};
    use crate::{config, repo};
    use crate::domain::primitives::{Debt, Length, LengthChange, LengthIncrement, Ratio, SignedLengthChange};
    use crate::repo::test::{CHAT_ID_KIND, start_postgres, USER_ID};

    #[tokio::test]
    async fn test_help_pussies() {
        {
            let invalid_perk = HelpPussiesPerk { coefficient: Ratio::literal(0.0) };
            assert!(!invalid_perk.enabled())
        }
        
        let perk = HelpPussiesPerk { coefficient: Ratio::literal(0.5) };
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let change_intent_positive_length = ChangeIntent { current_length: Length::new(1), base_increment: LengthIncrement::literal(1).into() };
        let change_intent_negative_length_positive_increment = ChangeIntent { current_length: Length::new(-1), base_increment: LengthIncrement::literal(1).into() };
        let change_intent_negative_length_negative_increment = ChangeIntent { current_length: Length::new(-1), base_increment: SignedLengthChange::new(-1).into() };
        
        assert!(perk.enabled());
        assert_eq!(perk.apply(&dick_id, change_intent_positive_length).await.0.value(), 0);
        assert_eq!(perk.apply(&dick_id, change_intent_negative_length_positive_increment).await.0.value(), 1);
        assert_eq!(perk.apply(&dick_id, change_intent_negative_length_negative_increment).await.0.value(), 1);
    }

    #[tokio::test]
    async fn test_loan_payout() {
        let (_container, db) = start_postgres().await;
        let loans = {
            let cfg = config::AppConfig {
                loan_payout_ratio: Ratio::literal(0.1),
                ..Default::default()
            };
            repo::Loans::new(db.clone(), &cfg)
        };

        {
            let users = repo::Users::new(db.clone());
            users.create_or_update(USER_ID, "")
                .await.expect("couldn't create a user");
            
            let dicks = repo::Dicks::new(db, Default::default());
            dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::Signed(SignedLengthChange::new(0)))
                .await.expect("couldn't create a dick");
        }

        let perk = LoanPayoutPerk { loans: loans.clone() };
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let change_intent_positive_increment = ChangeIntent { current_length: Length::new(1), base_increment: LengthIncrement::literal(10).into() };
        let change_intent_positive_increment_small = ChangeIntent { current_length: Length::new(1), base_increment: LengthIncrement::literal(2).into() };
        let change_intent_negative_increment = ChangeIntent { current_length: Length::new(1), base_increment: SignedLengthChange::new(-1).into() };

        assert!(perk.enabled());
        assert_eq!(perk.apply(&dick_id, change_intent_positive_increment).await.0.value(), 0);

        loans.borrow(USER_ID, &CHAT_ID_KIND, Debt::new(10))
            .await.expect("couldn't create a loan");

        assert_eq!(perk.apply(&dick_id, change_intent_positive_increment).await.0.value(), -1);
        let debt = loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the active loan")
            .expect("loan must be found")
            .debt;
        assert_eq!(debt, Debt::new(9));

        assert_eq!(perk.apply(&dick_id, change_intent_positive_increment_small).await.0.value(), 0);
        assert_eq!(perk.apply(&dick_id, change_intent_negative_increment).await.0.value(), 0);
        let debt = loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the active loan")
            .expect("loan must be found")
            .debt;
        assert_eq!(debt, Debt::new(9));
    }
}
