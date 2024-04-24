use anyhow::anyhow;
use sqlx::{Postgres, Transaction};
use teloxide::types::UserId;
use crate::repository;
use crate::repo::{ChatIdKind, ensure_only_one_row_updated};

repository!(Loans,
    pub async fn get_active_loan(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<u16> {
        sqlx::query_scalar!("SELECT debt FROM loans \
                                WHERE uid = $1 AND \
                                chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text) \
                                AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_loan| maybe_loan.map(|debt| debt as u16).unwrap_or_default())
            .map_err(Into::into)
    }
,
    pub async fn borrow(&self, uid: UserId, chat_id: &ChatIdKind, value: u16) -> anyhow::Result<Transaction<Postgres>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!("INSERT INTO Loans (chat_id, uid, debt) VALUES (\
                        (SELECT id FROM Chats WHERE chat_id = $1::bigint OR chat_instance = $1::text),\
                        $2, $3)",
                chat_id.value() as String, uid.0 as i64, value as i32)
            .execute(&mut *tx)
            .await
            .map(|_| tx)
            .map_err(Into::into)
    }
,
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
);
