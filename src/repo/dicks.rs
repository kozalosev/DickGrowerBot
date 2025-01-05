use anyhow::{anyhow, Context};
use futures::TryFutureExt;
use sqlx::{Executor, Pool, Postgres, Transaction};
use teloxide::types::UserId;
use crate::config::FeatureToggles;
use super::{ChatIdKind, ChatIdPartiality, Chats, UID};

#[derive(sqlx::FromRow, Debug)]
pub struct Dick {
    pub length: i32,
    pub owner_uid: UID,
    pub owner_name: String,
    pub grown_at: chrono::DateTime<chrono::Utc>,
    pub position: Option<i64>,
}

pub struct GrowthResult {
    pub new_length: i32,
    pub pos_in_top: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Dicks {
    pool: Pool<Postgres>,
    chats: Chats,
    features: FeatureToggles,
}

impl Dicks {
    pub fn new(pool: Pool<Postgres>, features: FeatureToggles) -> Self {
        Self {
            chats: Chats::new(pool.clone(), features),
            pool,
            features,
        }
    }

    #[tracing::instrument]
    pub async fn create_or_grow(&self, uid: UserId, chat_id: &ChatIdPartiality, increment: i32) -> anyhow::Result<GrowthResult> {
        let uid = uid.0 as i64;
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;
        let new_length = sqlx::query_scalar!(
            "INSERT INTO dicks(uid, chat_id, length, updated_at) VALUES ($1, $2, $3, current_timestamp)
                ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $3), updated_at = current_timestamp
                RETURNING length",
                uid, internal_chat_id, increment)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't upsert the dick of {uid} in {chat_id} with increment of {increment}"))?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await?;
        Ok(GrowthResult { new_length, pos_in_top })
    }

    #[tracing::instrument]
    pub async fn fetch_length(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<i32> {
        sqlx::query_scalar!("SELECT d.length FROM Dicks d \
                JOIN Chats c ON d.chat_id = c.id \
                WHERE uid = $1 AND \
                    c.chat_id = $2::bigint OR c.chat_instance = $2::text",
                uid.0 as i64, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(Option::unwrap_or_default)
            .context(format!("couldn't fetch length for {chat_id} and {uid}"))
    }

    #[tracing::instrument]
    pub async fn fetch_dick(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Option<Dick>> {
        sqlx::query_as!(Dick,
            r#"SELECT length, uid as owner_uid, name as owner_name, updated_at as grown_at, position FROM (
                 SELECT uid, name, d.length as length, updated_at, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                   FROM Dicks d
                   JOIN users using (uid)
                   JOIN Chats c ON d.chat_id = c.id
                   WHERE c.chat_id = $2::bigint OR c.chat_instance = $2::text
               ) AS _
               WHERE uid = $1"#,
                uid.0 as i64, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't fetch dick for {chat_id} and {uid}"))
    }

    #[tracing::instrument]
    pub async fn get_top(&self, chat_id: &ChatIdKind, offset: u32, limit: u16) -> anyhow::Result<Vec<Dick>> {
        sqlx::query_as!(Dick,
            r#"SELECT length, uid as owner_uid, name as owner_name, updated_at as grown_at,
                    ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                FROM dicks d
                JOIN users using (uid)
                JOIN chats c ON c.id = d.chat_id
                WHERE c.chat_id = $1::bigint OR c.chat_instance = $1::text
                OFFSET $2 LIMIT $3"#,
                chat_id.value() as String, offset as i64, limit as i32)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't get the top of {chat_id} with offset = {offset} and limit = {limit}"))
    }

    #[tracing::instrument]
    pub async fn set_dod_winner(&self, chat_id: &ChatIdPartiality, user_id: UserId, bonus: u16) -> anyhow::Result<Option<GrowthResult>> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;

        let mut tx = self.pool.begin().await?;
        let uid = user_id.0 as i64;
        let new_length = match Self::grow_no_attempts_check_internal(&mut *tx, internal_chat_id, uid, bonus as i32).await? {
            Some(length) => length,
            None => return Ok(None)
        };
        Self::insert_to_dod_table(&mut tx, internal_chat_id, uid).await?;
        tx.commit().await?;

        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await?;
        Ok(Some(GrowthResult { new_length, pos_in_top }))
    }

    #[tracing::instrument]
    pub async fn check_dick(&self, chat_id: &ChatIdKind, user_id: UserId, length: u16) -> anyhow::Result<bool> {
        sqlx::query_scalar!(r#"SELECT length >= $3 AS "enough!" FROM Dicks d
                JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                    AND uid = $2"#,
                chat_id.value() as String, user_id.0 as i64, length as i32)
            .fetch_optional(&self.pool)
            .map_ok(|opt| opt.unwrap_or(false))
            .await
            .context(format!("couldn't check the dick {chat_id}, {user_id} to have at least {length} cm"))
    }

    #[tracing::instrument]
    pub async fn move_length(&self, chat_id: &ChatIdPartiality, from: UserId, to: UserId, length: u16) -> anyhow::Result<(GrowthResult, GrowthResult)> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;

        let mut tx = self.pool.begin().await?;
        let length_from = Self::move_length_for_one_user(&mut tx, internal_chat_id, from.0, -(length as i32)).await?;
        let length_to = Self::move_length_for_one_user(&mut tx, internal_chat_id, to.0, length as i32).await?;
        tx.commit().await?;

        let pos_from = self.get_position_in_top(internal_chat_id, from.0 as i64).await?;
        let pos_to = self.get_position_in_top(internal_chat_id, to.0 as i64).await?;
        let gr_from = GrowthResult {
            new_length: length_from,
            pos_in_top: pos_from,
        };
        let gr_to = GrowthResult {
            new_length: length_to,
            pos_in_top: pos_to,
        };
        Ok((gr_from, gr_to))
    }

    #[tracing::instrument]
    async fn move_length_for_one_user(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: u64, change: i32) -> anyhow::Result<i32> {
        sqlx::query_scalar!("UPDATE Dicks SET length = (length + $3), bonus_attempts = (bonus_attempts + 1) WHERE chat_id = $1 AND uid = $2 RETURNING length",
                    chat_id_internal, user_id as i64, change)
            .fetch_one(&mut **tx)
            .await
            .context(format!("couldn't update the length by {change} for {chat_id_internal}, {user_id}"))
    }

    #[tracing::instrument]
    async fn get_position_in_top(&self, chat_id_internal: i64, uid: i64) -> anyhow::Result<Option<u64>> {
        if !self.features.top_unlimited {
            return Ok(None)
        }
        sqlx::query_scalar!(
                r#"SELECT position AS "position!" FROM (
                    SELECT uid, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                    FROM dicks
                    JOIN users using (uid)
                    WHERE chat_id = $1
                ) AS _
                WHERE uid = $2"#,
                chat_id_internal, uid)
            .fetch_one(&self.pool)
            .await
            .map(|pos| Some(pos as u64))
            .context(format!("couldn't get the top for {chat_id_internal} and {uid}"))
    }

    #[tracing::instrument]
    pub async fn grow_no_attempts_check(&self, chat_id: &ChatIdKind, user_id: UserId, change: i32) -> anyhow::Result<GrowthResult> {
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;
        let uid = user_id.0 as i64;
    
        let new_length = Self::grow_no_attempts_check_internal(&self.pool, chat_internal_id, uid, change).await?
            .ok_or(anyhow!("couldn't find a dick of ({chat_id}, {uid}) for some reason"))?;
        let pos_in_top = self.get_position_in_top(chat_internal_id, uid).await?;
        
        Ok(GrowthResult { new_length, pos_in_top })
    }

    #[tracing::instrument]
    pub(super) async fn grow_no_attempts_check_internal<'c, E>(executor: E, chat_id_internal: i64, user_id: i64, bonus: i32) -> anyhow::Result<Option<i32>>
    where E: Executor<'c, Database = Postgres>,
    {
        sqlx::query_scalar!(
            "UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $3)
                WHERE chat_id = $1 AND uid = $2
                RETURNING length",
                chat_id_internal, user_id, bonus)
            .fetch_optional(executor)
            .await
            .context(format!("couldn't grow the dick without attempts check for {chat_id_internal} and {user_id} by {bonus}"))
    }

    #[tracing::instrument]
    async fn insert_to_dod_table(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: i64) -> anyhow::Result<()> {
        sqlx::query!("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)",
                chat_id_internal, user_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't insert to DOD table for {chat_id_internal} and {user_id}"))?;
        Ok(())
    }
}
