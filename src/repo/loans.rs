use anyhow::anyhow;
use sqlx::Postgres;
use teloxide::types::UserId;

use crate::config;
use crate::repo::{ChatIdKind, Chats, Dicks, ensure_only_one_row_updated};

#[derive(Debug)]
pub struct Loan {
    pub debt: u16,
    pub payout_ratio: f32,
}

struct LoanEntity {
    debt: i32,
    payout_ratio: f32
}

impl TryFrom<LoanEntity> for Loan {
    type Error = std::num::TryFromIntError;

    fn try_from(value: LoanEntity) -> Result<Self, Self::Error> {
        Ok(Self {
            debt: value.debt.try_into()?,
            payout_ratio: value.payout_ratio
        })
    }
}

#[derive(Clone)]
pub struct Loans {
    pool: sqlx::Pool<Postgres>,
    chats: Chats,
    payout_ratio: f32,
}

impl Loans {
    pub fn new(pool: sqlx::Pool<Postgres>, cfg: &config::AppConfig) -> Self {
        let chats = Chats::new(pool.clone(), cfg.features);
        let payout_ratio = cfg.loan_payout_ratio;
        Self { pool, chats, payout_ratio }
    }

    pub async fn get_active_loan(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Option<Loan>> {
        let maybe_loan = sqlx::query_as!(LoanEntity,
            "SELECT debt, payout_ratio FROM loans \
                    WHERE uid = $1 AND \
                    chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text) \
                    AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await?
            .map(Loan::try_from)
            .transpose()?;
        Ok(maybe_loan)
    }

    pub async fn borrow(&self, uid: UserId, chat_id: &ChatIdKind, value: u16) -> anyhow::Result<()> {
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;
        let mut tx = self.pool.begin().await?;

        sqlx::query!("INSERT INTO Loans (chat_id, uid, debt, payout_ratio) VALUES ($1, $2, $3, $4)",
                chat_internal_id, uid.0 as i64, value as i32, self.payout_ratio)
            .execute(&mut *tx)
            .await?;
        Dicks::grow_no_attempts_check_internal(&mut *tx, chat_internal_id, uid.0 as i64, value.into()).await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn pay(&self, uid: UserId, chat_id: &ChatIdKind, value: u16) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Loans SET debt = debt - $3 \
                        WHERE uid = $1 AND \
                        chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text) \
                        AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String, value as i32)
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!(e))
            .and_then(ensure_only_one_row_updated)?;
        Ok(())
    }
}
