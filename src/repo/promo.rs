use std::fmt::Debug;
use anyhow::anyhow;
use sqlx::Postgres;
use strum_macros::Display;
use teloxide::types::UserId;
use crate::repository;

const PROMOCODE_ACTIVATIONS_PK: &str = "promo_code_activations_pkey";

pub struct ActivationResult {
    pub chats_affected: u64,
    pub bonus_length: i32,
}

#[derive(Debug, Display)]
#[strum(serialize_all = "snake_case")]
pub enum ActivationError {
    NoActivationsLeft,
    NoDicks,
    AlreadyActivated,
    Other(anyhow::Error)
}

impl <T: Into<anyhow::Error>> From<T> for ActivationError {
    fn from(value: T) -> Self {
        Self::Other(anyhow!(value))
    }
}

repository!(Promo,
    pub async fn activate(&self, user_id: UserId, code: &str) -> Result<ActivationResult, ActivationError> {
        let mut tx = self.pool.begin().await?;

        let bonus_length = Self::find_code_length_and_decr_capacity(&mut tx, code)
            .await?
            .ok_or(ActivationError::NoActivationsLeft)?;
        let chats_affected = Self::grow_dick(&mut tx, user_id, bonus_length).await?;
        if chats_affected < 1 {
            return Err(ActivationError::NoDicks)
        }
        Self::add_activation(&mut tx, user_id, code, chats_affected)
            .await
            .map_err(|err| {
                match err.downcast() {
                    Ok(sqlx::Error::Database(e)) => {
                        e.constraint()
                            .filter(|c| c == &PROMOCODE_ACTIVATIONS_PK)
                            .map(|_| ActivationError::AlreadyActivated)
                            .unwrap_or(ActivationError::Other(e.into()))
                    },
                    Ok(e) => ActivationError::Other(anyhow!(e)),
                    Err(e) => ActivationError::Other(e)
                }
            })?;

        tx.commit().await?;
        Ok(ActivationResult{ chats_affected, bonus_length })
    }
,
    async fn find_code_length_and_decr_capacity(tx: &mut sqlx::Transaction<'_, Postgres>, code: &str) -> anyhow::Result<Option<i32>> {
         sqlx::query_scalar!(
            "UPDATE Promo_Codes SET capacity = (capacity - 1)
                WHERE code = $1 AND capacity > 0 AND
                    (current_date BETWEEN since AND until
                    OR
                    current_date >= since AND until IS NULL)
                RETURNING bonus_length",
                code)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.into())
    }
,
    async fn grow_dick(tx: &mut sqlx::Transaction<'_, Postgres>, user_id: UserId, bonus: i32) -> anyhow::Result<u64> {
        let rows_affected = sqlx::query!("UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $2) WHERE uid = $1",
                user_id.0 as i64, bonus)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        Ok(rows_affected)
    }
,
    async fn add_activation(tx: &mut sqlx::Transaction<'_, Postgres>, uid: UserId, code: &str, affected_chats: u64) -> anyhow::Result<()> {
        let affected_chats: i32 = affected_chats.try_into()?;
        sqlx::query!("INSERT INTO Promo_Code_Activations (uid, code, affected_chats) VALUES ($1, $2, $3)",
                uid.0 as i64, code, affected_chats)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
