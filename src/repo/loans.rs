use anyhow::anyhow;
use teloxide::types::UserId;
use crate::repository;
use crate::repo::{ChatIdKind, ensure_only_one_row_updated};

repository!(Loans,
    pub async fn get_active_loan(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<u32> {
        sqlx::query_scalar!("SELECT left_to_pay FROM loans \
                                WHERE uid = $1 AND \
                                chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text) \
                                AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_loan| maybe_loan.map(|value| value as u32).unwrap_or_default())
            .map_err(|e| e.into())
    }
,
    pub async fn pay(&self, uid: UserId, chat_id: ChatIdKind, value: u32) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Loans SET left_to_pay = left_to_pay - $3 \
                        WHERE uid = $1 AND \
                        chat_id = (SELECT id FROM Chats WHERE chat_id = $2::bigint OR chat_instance = $2::text) \
                        AND repaid_at IS NULL",
                uid.0 as i64, chat_id.value() as String, value as i64)
            .execute(&self.pool)
            .await
            .map_err(|e| anyhow!(e))
            .and_then(ensure_only_one_row_updated)?;
        Ok(())
    }
);
