use autometrics::autometrics;
use std::fmt::Debug;
use anyhow::{anyhow, Context};
use sqlx::{FromRow, Postgres};
use crate::domain::primitives::{AffectedRows, LengthChange, PromoBonus, PromoCapacity, PromoCode, UserId};
use crate::repository;

const PROMOCODE_ACTIVATIONS_PK: &str = "promo_code_activations_pkey";

pub struct ActivationResult {
    pub chats_affected: AffectedRows,
    pub bonus_length: LengthChange,
}

#[derive(Debug, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
pub enum ActivationError {
    NotFound,
    NotStarted,
    Expired,
    Exhausted,
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
    found_code: PromoCode,
    bonus_length: PromoBonus,
    state: PromoCodeActiveState,
    capacity: PromoCapacity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
enum PromoCodeActiveState {
    Active,
    NotStarted,
    Ended,
}

repository!(Promo,
    #[cfg(test)]
    pub async fn create_promo_code(&self, p: PromoCodeParams) -> anyhow::Result<()> {
        sqlx::query!("INSERT INTO Promo_Codes (code, bonus_length, capacity) VALUES ($1, $2, $3)",
                p.code as PromoCode, p.bonus_length as PromoBonus, p.capacity as PromoCapacity)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = user_id.value(), code = %code))]
    pub async fn activate(&self, user_id: UserId, code: &PromoCode) -> Result<ActivationResult, ActivationError> {
        let mut tx = self.pool.begin().await?;

        let (found_code, bonus_length) = match Self::find_code_length_and_decr_capacity(&mut tx, code).await? {
            None => return Err(ActivationError::NotFound),
            Some(PromoCodeInfo { state: PromoCodeActiveState::NotStarted, .. }) => return Err(ActivationError::NotStarted),
            Some(PromoCodeInfo { state: PromoCodeActiveState::Ended, .. }) => return Err(ActivationError::Expired),
            Some(PromoCodeInfo { capacity, .. }) if capacity.is_zero() => return Err(ActivationError::Exhausted),
            Some(PromoCodeInfo { found_code, bonus_length, .. }) => (found_code, bonus_length),
        };
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
    /// `None` means no code by this name has ever existed. A code that has, but falls outside its
    /// `since`/`until` window, still matches, with the side of the window it is on as its `state`.
    /// The `capacity` returned is the row's own, whether or not this call could spend any of it.
    /// All of them are worth telling apart in the answer a player gets.
    ///
    /// `matched` takes the row lock and reads `capacity` and the window as they stand;
    /// `decremented` spends one unit of capacity, but only for a row that is active and has some,
    /// so neither an inactive nor an exhausted code is touched. It has nothing left to return once
    /// its own `code` isn't needed by the final `SELECT` any more, but Postgres still runs a
    /// data-modifying CTE to completion even unreferenced. `FOR NO KEY UPDATE` rather than `FOR
    /// UPDATE`: nothing here changes the row's key, so the weaker lock is enough and leaves room
    /// for a concurrent foreign-key check against it.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(code = %code))]
    async fn find_code_length_and_decr_capacity(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        code: &PromoCode,
    ) -> anyhow::Result<Option<PromoCodeInfo>> {
         sqlx::query_as!(PromoCodeInfo,
            r#"WITH matched AS (
                SELECT code, bonus_length, capacity,
                    CASE
                        WHEN current_date < since THEN 'not_started'
                        WHEN current_date > until THEN 'ended'
                        ELSE 'active'
                    END AS state
                FROM Promo_Codes
                WHERE lower(code) = lower($1)
                FOR NO KEY UPDATE
            ), decremented AS (
                UPDATE Promo_Codes SET capacity = capacity - 1
                WHERE code IN (SELECT code FROM matched WHERE state = 'active' AND capacity > 0)
            )
            SELECT code as "found_code: PromoCode", bonus_length as "bonus_length: PromoBonus",
                   state as "state!: PromoCodeActiveState", capacity as "capacity: PromoCapacity"
            FROM matched"#,
                code as &PromoCode)
            .fetch_optional(&mut **tx)
            .await
            .context(format!("couldn't find a promo code length of {code}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = user_id.value(), bonus = bonus.value()))]
    async fn grow_dicks(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        user_id: UserId,
        bonus: PromoBonus,
    ) -> anyhow::Result<AffectedRows> {
        let rows_affected = sqlx::query!("UPDATE Dicks SET length = (length + $2) WHERE uid = $1",
                user_id as UserId, i64::from(bonus.value()))
            .execute(&mut **tx)
            .await
            .context(format!("couldn't grow dicks of {user_id} by {bonus}"))?
            .rows_affected();
        Ok(AffectedRows::new(rows_affected))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), code = %code, affected_chats = affected_chats.value()))]
    async fn add_activation(
        tx: &mut sqlx::Transaction<'_, Postgres>,
        uid: UserId,
        code: &PromoCode,
        affected_chats: AffectedRows,
    ) -> anyhow::Result<()> {
        let affected_chats: i32 = affected_chats.value().try_into()?;
        sqlx::query!("INSERT INTO Promo_Code_Activations (uid, code, affected_chats, activated_at) VALUES ($1, $2, $3, current_timestamp)",
                uid as UserId, code as &PromoCode, affected_chats)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't insert a promo code activation for {uid} and {code} with {affected_chats} affected chats"))?;
        Ok(())
    }
);
