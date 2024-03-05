use async_trait::async_trait;
use chashmap::CHashMap;
use num_traits::ToPrimitive;
use sqlx::{Pool, Postgres};
use crate::handlers::utils::{AdditionalChange, ChangeIntent, DickId, Perk};
use crate::{config, repo};
use crate::config::FeatureToggles;

pub fn all(pool: &Pool<Postgres>, features: FeatureToggles) -> Vec<Box<dyn Perk>> {
    let help_pussies_coef = config::get_env_value_or_default("HELP_PUSSIES_COEFF", 0.0);
    let payout_coefficient = config::get_env_value_or_default("LOAN_WRITEOFF_COEFF", 0.0);
    let loans = repo::Loans::new(pool.clone(), features);
    
    vec![
        Box::new(HelpPussiesPerk {
            coefficient: help_pussies_coef,
        }),
        Box::new(LoanPayoutPerk::new(
            payout_coefficient,
            loans,
        ))
    ]
}

struct HelpPussiesPerk {
    coefficient: f64
}

#[async_trait]
impl Perk for HelpPussiesPerk {
    fn name(&self) -> &'static str {
        "help-pussies"
    }

    fn enabled(&self) -> bool {
        self.coefficient > 0.0
    }

    async fn active(&self, _: &DickId, change_intent: ChangeIntent) -> bool {
        change_intent.current_length < 0
    }

    async fn apply(&self, _: &DickId, change_intent: ChangeIntent) -> AdditionalChange {
        let current_length = change_intent.current_length.to_f64().expect("conversion is always Some");
        let change = (self.coefficient * current_length).ceil() as i32;
        let ac = if change_intent.base_increment.is_positive() {
            change
        } else {
            -change
        };
        AdditionalChange(ac)
    }
}

struct LoanPayoutPerk {
    payout_coefficient: f64,
    loans: repo::Loans,
    debts: CHashMap<DickId, u16>
}

impl LoanPayoutPerk {
    fn new(payout_coefficient: f64, loans: repo::Loans) -> Self {
        Self {
            payout_coefficient, loans,
            debts: CHashMap::new()
        }
    }
}

#[async_trait]
impl Perk for LoanPayoutPerk {
    fn name(&self) -> &'static str {
        "loan-payout"
    }

    fn enabled(&self) -> bool {
        (0.0..=1.0).contains(&self.payout_coefficient)
    }

    async fn active(&self, dick_id: &DickId, _: ChangeIntent) -> bool {
        let debt = self.loans.get_active_loan(dick_id.0, &dick_id.1)
            .await
            .inspect_err(|e| log::error!("couldn't check if a perk is active: {e}"))
            .unwrap_or(0);
        if debt > 0 {
            self.debts.insert(dick_id.clone(), debt);
            true
        } else {
            self.debts.remove(dick_id);
            false
        }            
    }

    async fn apply(&self, dick_id: &DickId, change_intent: ChangeIntent) -> AdditionalChange {
        let payout = if change_intent.base_increment.is_positive() {
            let base_increment = change_intent.base_increment.to_f64().expect("conversion gives always Some");
            let payout = (base_increment * self.payout_coefficient).floor() as u16;
            
            let debt = self.debts.remove(dick_id).expect("debt must be in the hashmap");
            payout.min(debt)
        } else {
            0
        };
        match self.loans.pay(dick_id.0, &dick_id.1, payout).await {
            Ok(()) => AdditionalChange(change_intent.base_increment - i32::from(payout)),
            Err(e) => {
                log::error!("couldn't pay for the loan ({dick_id}): {e}");
                AdditionalChange(0)
            }
        }
    }
}
