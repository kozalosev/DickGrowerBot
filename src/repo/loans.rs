use anyhow::anyhow;
use derive_more::Constructor;
use sqlx::{Postgres, Transaction};
use teloxide::types::UserId;
use crate::repo::{ChatIdKind, ensure_only_one_row_updated};

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

#[derive(Clone, Constructor)]
pub struct Loans {
    pool: sqlx::Pool<Postgres>,
    payout_ratio: f32
}

impl Loans {    
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

    pub async fn borrow(&self, uid: UserId, chat_id: &ChatIdKind, value: u16) -> anyhow::Result<Transaction<Postgres>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!("INSERT INTO Loans (chat_id, uid, debt, payout_ratio) VALUES (\
                        (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text),\
                        $2, $3, $4)",
                chat_id.value() as String, uid.0 as i64, value as i32, self.payout_ratio)
            .execute(&mut *tx)
            .await
            .map(|_| tx)
            .map_err(Into::into)
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
