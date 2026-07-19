use anyhow::Context;
use sqlx::{Postgres, Transaction};

use crate::config;
use crate::domain::objects::Loan;
use crate::domain::primitives::{Debt, LengthChange, LoanId, LoanPayout, Ratio, UserId};
use crate::domain::primitives::chat::InternalChatId;
use crate::repo::{ensure_only_one_row_updated, ChatIdKind, Chats, Dicks};

struct LoanEntity {
    id: LoanId,
    debt: Debt,
    // the column is REAL (f32) in the database; converted to Ratio at the boundary
    payout_ratio: f32,
}

impl From<LoanEntity> for Loan {
    fn from(entity: LoanEntity) -> Self {
        Loan {
            debt: entity.debt,
            payout_ratio: Ratio::new(entity.payout_ratio.into())
                .expect("payout_ratio, fetched from the database, must be a valid ratio"),
        }
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
            r#"SELECT id AS "id: LoanId", debt AS "debt: Debt", payout_ratio FROM loans
                    WHERE uid = $1 AND
                    chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text)
                    AND repaid_at IS NULL"#,
                uid as UserId, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get an active loan for {chat_id} and {uid}"))?
            .map(Loan::from);
        Ok(maybe_loan)
    }

    pub async fn borrow(&self, user_id: UserId, chat_id: &ChatIdKind, value: Debt) -> anyhow::Result<BorrowResult> {
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;
        let mut tx = self.pool.begin().await?;

        // re-check the eligibility at the moment of the borrowing itself, locking the row
        // to serialize concurrent attempts (e.g. several stale confirmation buttons)
        let length = fetch_length_locked(&mut tx, user_id, chat_internal_id).await?;
        if length.map(|len| len >= 0).unwrap_or(true) {
            return Ok(BorrowResult::NotEligible)
        }

        match get_active_loan(&mut tx, user_id, chat_internal_id).await? {
            Some(LoanEntity { id, .. }) => refinance_loan(&mut tx, id, value, self.payout_ratio).await?,
            None => create_loan(&mut tx, chat_internal_id, user_id, value, self.payout_ratio).await?
        };
        let borrowed_length = LengthChange::signed(value.value());
        Dicks::grow_no_attempts_check_internal(&mut *tx, chat_internal_id, user_id, borrowed_length).await?;

        tx.commit().await?;
        Ok(BorrowResult::Granted)
    }

    pub async fn pay(&self, uid: UserId, chat_id: &ChatIdKind, value: LoanPayout) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Loans SET debt = debt - $3
                        WHERE uid = $1 AND
                        chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text)
                        AND repaid_at IS NULL",
                uid as UserId, chat_id.value() as String, value as LoanPayout)
            .execute(&self.pool)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't pay for a loan: {chat_id}, {uid}, {value}"))
    }
}

async fn fetch_length_locked(tx: &mut Transaction<'_, Postgres>, uid: UserId, chat_internal_id: InternalChatId) -> anyhow::Result<Option<i64>> {
    sqlx::query_scalar!("SELECT length FROM Dicks WHERE chat_id = $1 AND uid = $2 FOR UPDATE",
            chat_internal_id as InternalChatId, uid as UserId)
        .fetch_optional(&mut **tx)
        .await
        .context(format!("couldn't fetch and lock the length for internal {chat_internal_id} and {uid}"))
}

async fn get_active_loan(tx: &mut Transaction<'_, Postgres>, uid: UserId, chat_internal_id: InternalChatId) -> anyhow::Result<Option<LoanEntity>> {
    let maybe_loan = sqlx::query_as!(LoanEntity,
            r#"SELECT id AS "id: LoanId", debt AS "debt: Debt", payout_ratio FROM loans
                    WHERE uid = $1 AND chat_id = $2
                    AND repaid_at IS NULL"#,
                uid as UserId, chat_internal_id as InternalChatId)
        .fetch_optional(&mut **tx)
        .await
        .context(format!("couldn't get an active loan for internal {chat_internal_id} and {uid}"))?;
    Ok(maybe_loan)
}

async fn create_loan(tx: &mut Transaction<'_, Postgres>, chat_internal_id: InternalChatId, uid: UserId, value: Debt, payout_ratio: Ratio) -> anyhow::Result<()> {
    sqlx::query!("INSERT INTO Loans (chat_id, uid, debt, payout_ratio) VALUES ($1, $2, $3, $4)",
                chat_internal_id as InternalChatId, uid as UserId, value as Debt, payout_ratio.value() as f32)
        .execute(&mut **tx)
        .await
        .map(ensure_only_one_row_updated)
        .context(format!("couldn't create a loan for {chat_internal_id} and {uid} with value of {value}"))?
}

async fn refinance_loan(tx: &mut Transaction<'_, Postgres>, id: LoanId, value: Debt, payout_ratio: Ratio) -> anyhow::Result<()> {
    sqlx::query!("UPDATE Loans l SET debt = l.debt + $2, payout_ratio = $3 WHERE id = $1",
                id as LoanId, value as Debt, payout_ratio.value() as f32)
        .execute(&mut **tx)
        .await
        .map(ensure_only_one_row_updated)
        .context(format!("couldn't update the loan with id = {id} (additional value is {value})"))?
}
