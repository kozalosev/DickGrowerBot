use std::fmt::Debug;
use anyhow::{anyhow, Context};
use sqlx::{FromRow, Postgres};
use crate::domain::primitives::{AffectedRows, LengthChange, PromoBonus, UserId};
#[cfg(test)]
use crate::domain::primitives::{PromoCapacity, PromoCode};
use crate::repository;

const PROMOCODE_ACTIVATIONS_PK: &str = "promo_code_activations_pkey";

pub struct ActivationResult {
    pub chats_affected: AffectedRows,
    pub bonus_length: LengthChange,
}

#[derive(Debug, strum_macros::Display)]
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

#[cfg(test)]
pub struct PromoCodeParams {
    pub code: PromoCode,
    pub bonus_length: PromoBonus,
    pub capacity: PromoCapacity,
}

#[derive(FromRow)]
struct PromoCodeInfo {
    found_code: String,
    bonus_length: PromoBonus,
}

repository!(Promo,
    #[cfg(test)]
    pub async fn create(&self, p: PromoCodeParams) -> anyhow::Result<()> {
        sqlx::query!("INSERT INTO Promo_Codes (code, bonus_length, capacity) VALUES ($1, $2, $3)",
                p.code as PromoCode, p.bonus_length as PromoBonus, p.capacity as PromoCapacity)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
,
    pub async fn activate(&self, user_id: UserId, code: &str) -> Result<ActivationResult, ActivationError> {
        let mut tx = self.pool.begin().await?;

        let PromoCodeInfo { found_code, bonus_length } = Self::find_code_length_and_decr_capacity(&mut tx, code)
            .await?
            .ok_or(ActivationError::NoActivationsLeft)?;
        // The bonus may be negative: such a promo code shrinks the dick instead of growing it.
        let bonus = LengthChange::signed(bonus_length.value().into());
        let chats_affected = Self::grow_dicks(&mut tx, user_id, bonus_length).await?;
        if chats_affected.zero() {
            return Err(ActivationError::NoDicks)
        }
        Self::add_activation(&mut tx, user_id, &found_code, chats_affected)
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
        Ok(ActivationResult{ chats_affected, bonus_length: bonus })
    }
,
    async fn find_code_length_and_decr_capacity(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        code: &str,
    ) -> anyhow::Result<Option<PromoCodeInfo>> {
         sqlx::query_as!(PromoCodeInfo,
            "UPDATE Promo_Codes SET capacity = (capacity - 1)
                WHERE lower(code) = lower($1) AND capacity > 0 AND
                    (current_date BETWEEN since AND until
                    OR
                    current_date >= since AND until IS NULL)
                RETURNING bonus_length as \"bonus_length: PromoBonus\", code as found_code",
                code)
            .fetch_optional(&mut **tx)
            .await
            .context(format!("couldn't find a promo code length of {code}"))
    }
,
    async fn grow_dicks(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        user_id: UserId,
        bonus: PromoBonus,
    ) -> anyhow::Result<AffectedRows> {
        let rows_affected = sqlx::query!("UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $2) WHERE uid = $1",
                user_id as UserId, i64::from(bonus.value()))
            .execute(&mut **tx)
            .await
            .context(format!("couldn't grow dicks of {user_id} by {bonus}"))?
            .rows_affected();
        Ok(AffectedRows::new(rows_affected))
    }
,
    async fn add_activation(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        uid: UserId,
        code: &str,
        affected_chats: AffectedRows,
    ) -> anyhow::Result<()> {
        let affected_chats: i32 = affected_chats.value().try_into()?;
        sqlx::query!("INSERT INTO Promo_Code_Activations (uid, code, affected_chats, activated_at) VALUES ($1, $2, $3, current_timestamp)",
                uid as UserId, code, affected_chats)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't insert a promo code activation for {uid} and {code} with {affected_chats} affected chats"))?;
        Ok(())
    }
);
