use anyhow::Context;
use sqlx::{Postgres, Transaction};
use teloxide::types::UserId;

use crate::config;
use crate::repo::{ChatIdKind, Chats, Dicks, ensure_only_one_row_updated};

#[derive(Debug)]
pub struct Loan {
    pub debt: u16,
    pub payout_ratio: f32,
}

struct LoanEntity {
    id: i32,
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

#[derive(Debug, PartialEq)]
pub enum BorrowResult {
    Granted,
    NotEligible,
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

    pub async fn borrow(&self, user_id: UserId, chat_id: &ChatIdKind, value: u16) -> anyhow::Result<BorrowResult> {
        let uid = user_id.0 as i64;
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;
        let mut tx = self.pool.begin().await?;

        // re-check the eligibility at the moment of the borrowing itself, locking the row
        // to serialize concurrent attempts (e.g. several stale confirmation buttons)
        let length = fetch_length_locked(&mut tx, uid, chat_internal_id).await?;
        if length.map(|len| len >= 0).unwrap_or(true) {
            return Ok(BorrowResult::NotEligible)
        }

        match get_active_loan(&mut tx, user_id, chat_internal_id).await? {
            Some(LoanEntity { id, .. }) => refinance_loan(&mut tx, id, value, self.payout_ratio).await?,
            None => create_loan(&mut tx, chat_internal_id, uid, value, self.payout_ratio).await?
        };
        Dicks::grow_no_attempts_check_internal(&mut *tx, chat_internal_id, uid, value.into()).await?;

        tx.commit().await?;
        Ok(BorrowResult::Granted)
    }

    pub async fn pay(&self, uid: UserId, chat_id: &ChatIdKind, value: u16) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Loans SET debt = debt - $3
                        WHERE uid = $1 AND
                        chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text)
                        AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String, value as i32)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't pay for a loan: {chat_id}, {uid}, {value}"))
    }
}

async fn fetch_length_locked(tx: &mut Transaction<'_, Postgres>, uid: i64, chat_internal_id: i64) -> anyhow::Result<Option<i32>> {
    sqlx::query_scalar!("SELECT length FROM Dicks WHERE chat_id = $1 AND uid = $2 FOR UPDATE",
            chat_internal_id, uid)
        .fetch_optional(&mut **tx)
        .await
        .context(format!("couldn't fetch and lock the length for internal {chat_internal_id} and {uid}"))
}

async fn get_active_loan(tx: &mut Transaction<'_, Postgres>, uid: UserId, chat_internal_id: i64) -> anyhow::Result<Option<LoanEntity>> {
    let maybe_loan = sqlx::query_as!(LoanEntity,
            "SELECT id, debt, payout_ratio FROM loans
                    WHERE uid = $1 AND chat_id = $2
                    AND repaid_at IS NULL",
                uid.0 as i64, chat_internal_id)
        .fetch_optional(&mut **tx)
        .await
        .context(format!("couldn't get an active loan for internal {chat_internal_id} and {uid}"))?;
    Ok(maybe_loan)
}

async fn create_loan(tx: &mut Transaction<'_, Postgres>, chat_internal_id: i64, uid: i64, value: u16, payout_ratio: f32) -> anyhow::Result<()> {
    sqlx::query!("INSERT INTO Loans (chat_id, uid, debt, payout_ratio) VALUES ($1, $2, $3, $4)",
                chat_internal_id, uid, value as i32, payout_ratio)
        .execute(&mut **tx)
        .await
        .map(ensure_only_one_row_updated)
        .context(format!("couldn't create a loan for {chat_internal_id} and {uid} with value of {value}"))?
}

async fn refinance_loan(tx: &mut Transaction<'_, Postgres>, id: i32, value: u16, payout_ratio: f32) -> anyhow::Result<()> {
    sqlx::query!("UPDATE Loans l SET debt = l.debt + $2, payout_ratio = $3 WHERE id = $1",
                id, value as i32, payout_ratio)
        .execute(&mut **tx)
        .await
        .map(ensure_only_one_row_updated)
        .context(format!("couldn't update the loan with id = {id} (additional value is {value})"))?
}
