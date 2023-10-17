use std::fmt::Debug;
use anyhow::anyhow;
use sqlx::Postgres;
use sqlx::Row;
use strum_macros::Display;
use teloxide::types::UserId;
use super::UID;
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
    pub async fn activate(&self, uid: UserId, code: &str) -> Result<ActivationResult, ActivationError> {
        let mut tx = self.pool.begin().await?;

        let uid = uid.0.try_into()?;
        let bonus_length = Self::find_code_length_and_decr_capacity(&mut tx, code)
            .await?
            .ok_or(ActivationError::NoActivationsLeft)?;
        let chats_affected = Self::grow_dick(&mut tx, uid, bonus_length).await?;
        if chats_affected < 1 {
            return Err(ActivationError::NoDicks)
        }
        Self::add_activation(&mut tx, uid, code, chats_affected)
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
         let length = sqlx::query(
            "UPDATE Promo_Codes SET capacity = (capacity - 1)
                WHERE code = $1 AND capacity > 0 AND
                    (current_date BETWEEN since AND until
                    OR
                    current_date >= since AND until IS NULL)
                RETURNING bonus_length")
            .bind(code)
            .fetch_optional(&mut **tx)
            .await?
            .and_then(|r| r.try_get("bonus_length").ok());
         Ok(length)
    }
,
    async fn grow_dick(tx: &mut sqlx::Transaction<'_, Postgres>, uid: UID, bonus: i32) -> anyhow::Result<u64> {
        let rows_affected = sqlx::query("UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $2) WHERE uid = $1")
            .bind(uid.0)
            .bind(bonus)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        Ok(rows_affected)
    }
,
    async fn add_activation(tx: &mut sqlx::Transaction<'_, Postgres>, uid: UID, code: &str, affected_chats: u64) -> anyhow::Result<()> {
        let affected_chats: i64 = affected_chats.try_into()?;
        sqlx::query("INSERT INTO Promo_Code_Activations (uid, code, affected_chats) VALUES ($1, $2, $3)")
            .bind(uid.0)
            .bind(code)
            .bind(affected_chats)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
