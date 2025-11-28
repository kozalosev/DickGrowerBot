use anyhow::Context;
use sqlx::{Postgres, Transaction};

use crate::config;
use crate::domain::primitives::{Debt, LoanId, Ratio, UserId};
use crate::domain::primitives::chat::InternalChatId;
use crate::repo::{ChatIdKind, Chats, Dicks, ensure_only_one_row_updated};

#[derive(Debug)]
pub struct Loan {
    pub debt: Debt,
    pub payout_ratio: Ratio,
}

struct LoanEntity {
    id: LoanId,
    debt: Debt,
    payout_ratio: Ratio,
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
    payout_ratio: Ratio,
}

impl Loans {
    pub fn new(pool: sqlx::Pool<Postgres>, cfg: &config::AppConfig) -> Self {
        let chats = Chats::new(pool.clone(), cfg.features);
        let payout_ratio = cfg.loan_payout_ratio;
        Self { pool, chats, payout_ratio }
    }

    pub async fn get_active_loan(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Option<Loan>> {
        let maybe_loan = sqlx::query_as!(LoanEntity,
            "SELECT id, debt, payout_ratio FROM loans
                    WHERE uid = $1 AND
                    chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text)
                    AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get an active loan for {chat_id} and {uid}"))?
            .map(Loan::try_from)
            .transpose()
            .context(format!("couldn't convert the loan for {chat_id} and {uid}"))?;
        Ok(maybe_loan)
    }

    pub async fn borrow(&self, user_id: UserId, chat_id: &ChatIdKind, value: Debt) -> anyhow::Result<()> {
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;
        let signed_value: i32 = value.try_into()?;
        let mut tx = self.pool.begin().await?;

        match get_active_loan(&mut tx, user_id, chat_internal_id).await? {
            Some(LoanEntity { id, .. }) => refinance_loan(&mut tx, id, signed_value, self.payout_ratio).await?,
            None => create_loan(&mut tx, chat_internal_id, uid, signed_value, self.payout_ratio).await?
        };
        Dicks::grow_no_attempts_check_internal(&mut *tx, chat_internal_id, user_id, signed_value).await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn pay(&self, uid: UserId, chat_id: &ChatIdKind, value: u32) -> anyhow::Result<()> {
        let signed_value: i32 = value.try_into()?;
        sqlx::query!("UPDATE Loans SET debt = debt - $3
                        WHERE uid = $1 AND
                        chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text)
                        AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String, signed_value)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't pay for a loan: {chat_id}, {uid}, {value}"))
    }
}

async fn get_active_loan(tx: &mut Transaction<'_, Postgres>, uid: UserId, chat_internal_id: InternalChatId) -> anyhow::Result<Option<LoanEntity>> {
    let maybe_loan = sqlx::query_as!(LoanEntity,
            "SELECT id, debt, payout_ratio FROM loans
                    WHERE uid = $1 AND chat_id = $2
                    AND repaid_at IS NULL",
                uid.0 as i64, *chat_internal_id)
        .fetch_optional(&mut **tx)
        .await
        .context(format!("couldn't get an active loan for internal {chat_internal_id} and {uid}"))?;
    Ok(maybe_loan)
}

async fn create_loan(tx: &mut Transaction<'_, Postgres>, chat_internal_id: i64, uid: i64, value: Debt, payout_ratio: f32) -> anyhow::Result<()> {
    sqlx::query!("INSERT INTO Loans (chat_id, uid, debt, payout_ratio) VALUES ($1, $2, $3, $4)",
                chat_internal_id, uid, *value, payout_ratio)
        .execute(&mut **tx)
        .await
        .map(ensure_only_one_row_updated)
        .context(format!("couldn't create a loan for {chat_internal_id} and {uid} with value of {value}"))?
}

async fn refinance_loan(tx: &mut Transaction<'_, Postgres>, id: i64, value: Debt, payout_ratio: f32) -> anyhow::Result<()> {
    sqlx::query!("UPDATE Loans l SET debt = l.debt + $2, payout_ratio = $3 WHERE id = $1",
                id, *value, payout_ratio)
        .execute(&mut **tx)
        .await
        .map(ensure_only_one_row_updated)
        .context(format!("couldn't update the loan with id = {id} (additional value is {value})"))?
}
