use async_trait::async_trait;
use num_traits::ToPrimitive;
use sqlx::{Pool, Postgres};
use crate::handlers::utils::{AdditionalChange, ChangeIntent, ConfigurablePerk, DickId, Perk};
use crate::{config, repo};

pub fn all(pool: &Pool<Postgres>, cfg: &config::AppConfig) -> Vec<Box<dyn Perk>> {
    let help_pussies_coef = config::get_env_value_or_default("HELP_PUSSIES_COEF", 0.0);
    let loans = repo::Loans::new(pool.clone(), cfg.loan_payout_ratio);
    
    vec![
        Box::new(HelpPussiesPerk {
            coefficient: help_pussies_coef,
        }),
        Box::new(LoanPayoutPerk { loans })
    ]
}

pub struct HelpPussiesPerk {
    coefficient: f64
}

#[async_trait]
impl Perk for HelpPussiesPerk {
    fn name(&self) -> &str {
        "help-pussies"
    }

    async fn apply(&self, _: &DickId, change_intent: ChangeIntent) -> AdditionalChange {
        if change_intent.current_length >= 0 {
            return AdditionalChange(0)
        }
        
        let current_deepness = change_intent.current_length.abs()
            .to_f64().expect("conversion is always Some");
        let change = (self.coefficient * current_deepness).round() as i32;
        AdditionalChange(change)
    }

    fn enabled(&self) -> bool {
        self.coefficient > 0.0
    }
}

impl ConfigurablePerk for HelpPussiesPerk {
    type Config = f64;

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
            None => return AdditionalChange(0)
        };

        let payout = if change_intent.base_increment.is_positive() {
            let base_increment = change_intent.base_increment as f32;
            let payout = (base_increment * payout_coefficient).round() as u16;
            payout.min(debt)
        } else {
            0
        };
        match self.loans.pay(dick_id.0, &dick_id.1, payout).await {
            Ok(()) => AdditionalChange(-i32::from(payout)),
            Err(e) => {
                log::error!("couldn't pay {payout} cm for the loan ({dick_id}): {e}");
                AdditionalChange(0)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use testcontainers::clients;
    use crate::handlers::perks::{HelpPussiesPerk, LoanPayoutPerk};
    use crate::handlers::utils::{ChangeIntent, DickId, Perk};
    use crate::repo;
    use crate::repo::test::{CHAT_ID_KIND, start_postgres, USER_ID};

    #[tokio::test]
    async fn test_help_pussies() {
        {
            let invalid_perk = HelpPussiesPerk { coefficient: 0.0 };
            assert!(!invalid_perk.enabled())
        }
        
        let perk = HelpPussiesPerk { coefficient: 0.5 };
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let change_intent_positive_length = ChangeIntent { current_length: 1, base_increment: 1 };
        let change_intent_negative_length_positive_increment = ChangeIntent { current_length: -1, base_increment: 1 };
        let change_intent_negative_length_negative_increment = ChangeIntent { current_length: -1, base_increment: -1 };
        
        assert!(perk.enabled());
        assert_eq!(perk.apply(&dick_id, change_intent_positive_length).await.0, 0);
        assert_eq!(perk.apply(&dick_id, change_intent_negative_length_positive_increment).await.0, 1);
        assert_eq!(perk.apply(&dick_id, change_intent_negative_length_negative_increment).await.0, 1);
    }

    #[tokio::test]
    async fn test_loan_payout() {
        let docker = clients::Cli::default();
        let (_container, db) = start_postgres(&docker).await;
        let loans = repo::Loans::new(db.clone(), 0.1);

        {
            let users = repo::Users::new(db.clone());
            users.create_or_update(USER_ID, "")
                .await.expect("couldn't create a user");
            
            let dicks = repo::Dicks::new(db, Default::default());
            dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), 0)
                .await.expect("couldn't create a dick");
        }

        let perk = LoanPayoutPerk { loans: loans.clone() };
        let dick_id = DickId(USER_ID, CHAT_ID_KIND);
        let change_intent_positive_increment = ChangeIntent { current_length: 1, base_increment: 10 };
        let change_intent_positive_increment_small = ChangeIntent { current_length: 1, base_increment: 2 };
        let change_intent_negative_increment = ChangeIntent { current_length: 1, base_increment: -1 };

        assert!(perk.enabled());
        assert_eq!(perk.apply(&dick_id, change_intent_positive_increment).await.0, 0);

        loans.borrow(USER_ID, &CHAT_ID_KIND, 10)
            .await.expect("couldn't create a loan")
            .commit()
            .await.expect("couldn't commit the creation of a loan");

        assert_eq!(perk.apply(&dick_id, change_intent_positive_increment).await.0, -1);
        let debt = loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the active loan")
            .expect("loan must be found")
            .debt;
        assert_eq!(debt, 9);

        assert_eq!(perk.apply(&dick_id, change_intent_positive_increment_small).await.0, 0);
        assert_eq!(perk.apply(&dick_id, change_intent_negative_increment).await.0, 0);
        let debt = loans.get_active_loan(USER_ID, &CHAT_ID_KIND)
            .await.expect("couldn't fetch the active loan")
            .expect("loan must be found")
            .debt;
        assert_eq!(debt, 9);
    }
}
