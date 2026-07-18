use anyhow::{anyhow, Context};
use futures::TryFutureExt;
use sqlx::{Executor, Pool, Postgres, Transaction};
use crate::config::FeatureToggles;
use crate::domain::objects::{Dick, GrowthResult};
use crate::domain::primitives::{Bet, LengthChange, Limit, Offset, UserId, Position, Length};
use crate::domain::primitives::chat::{ChatIdPartiality, ChatIdKind, InternalChatId};
use super::Chats;

/// The database projection of a [`Dick`]. `position` is a `ROW_NUMBER()` (a plain `int8`),
/// so it's decoded as `i64` here and converted to the `Position` domain type at this boundary
/// (`Position` wraps a `u64` and isn't a database type).
#[derive(sqlx::FromRow)]
struct DickEntity {
    length: Length,
    owner_uid: UserId,
    owner_name: String,
    grown_at: chrono::DateTime<chrono::Utc>,
    position: Option<i64>,
}

impl From<DickEntity> for Dick {
    fn from(entity: DickEntity) -> Self {
        Self {
            length: entity.length,
            owner_uid: entity.owner_uid,
            owner_name: entity.owner_name,
            grown_at: entity.grown_at,
            position: entity.position.map(|pos| Position::new(pos as u64)),
        }
    }
}

#[derive(Clone)]
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

    pub async fn create_or_grow(&self, uid: UserId, chat_id: &ChatIdPartiality, increment: LengthChange) -> anyhow::Result<GrowthResult> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;
        let new_length = sqlx::query_scalar!(
            "INSERT INTO dicks(uid, chat_id, length, updated_at) VALUES ($1, $2, $3, current_timestamp)
                ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $3), updated_at = current_timestamp
                RETURNING length",
                uid as UserId, internal_chat_id as InternalChatId, increment.value() as i64)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't upsert the dick of {uid} in {chat_id} with increment of {increment}"))?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await?;
        Ok(GrowthResult { new_length: Length::new(new_length), pos_in_top })
    }

    pub async fn fetch_length(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Length> {
        sqlx::query_scalar!("SELECT d.length FROM Dicks d \
                JOIN Chats c ON d.chat_id = c.id \
                WHERE uid = $1 AND \
                    (c.chat_id = $2::bigint OR c.chat_instance = $2::text)",
                uid as UserId, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_length| maybe_length.map(Length::new).unwrap_or_default())
            .context(format!("couldn't fetch length for {chat_id} and {uid}"))
    }

    pub async fn fetch_dick(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Option<Dick>> {
        sqlx::query_as!(DickEntity,
            r#"SELECT length AS "length: Length", uid AS "owner_uid: UserId", name as owner_name, updated_at as grown_at, position FROM (
                 SELECT uid, name, d.length as length, updated_at, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                   FROM Dicks d
                   JOIN users using (uid)
                   JOIN Chats c ON d.chat_id = c.id
                   WHERE c.chat_id = $2::bigint OR c.chat_instance = $2::text
               ) AS _
               WHERE uid = $1"#,
                uid as UserId, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_dick| maybe_dick.map(Dick::from))
            .context(format!("couldn't fetch dick for {chat_id} and {uid}"))
    }

    pub async fn get_top(&self, chat_id: &ChatIdKind, offset: Offset, limit: Limit) -> anyhow::Result<Vec<Dick>> {
        sqlx::query_as!(DickEntity,
            r#"SELECT length AS "length: Length", uid AS "owner_uid: UserId", name as owner_name, updated_at as grown_at,
                    ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                FROM dicks d
                JOIN users using (uid)
                JOIN chats c ON c.id = d.chat_id
                WHERE c.chat_id = $1::bigint OR c.chat_instance = $1::text
                OFFSET $2 LIMIT $3"#,
                chat_id.value() as String, offset as Offset, limit as Limit)
            .fetch_all(&self.pool)
            .await
            .map(|dicks| dicks.into_iter().map(Dick::from).collect())
            .context(format!("couldn't get the top of {chat_id} with offset = {offset} and limit = {limit}"))
    }

    pub async fn set_dod_winner(&self, chat_id: &ChatIdPartiality, user_id: UserId, bonus: LengthChange) -> anyhow::Result<Option<GrowthResult>> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;

        let mut tx = self.pool.begin().await?;
        let new_length = match Self::grow_no_attempts_check_internal(&mut *tx, internal_chat_id, user_id, bonus).await? {
            Some(length) => length,
            None => return Ok(None)
        };
        Self::insert_to_dod_table(&mut tx, internal_chat_id, user_id).await?;
        tx.commit().await?;

        let pos_in_top = self.get_position_in_top(internal_chat_id, user_id).await?;
        Ok(Some(GrowthResult { new_length, pos_in_top }))
    }

    pub async fn check_dick(&self, chat_id: &ChatIdKind, user_id: UserId, length: Bet) -> anyhow::Result<bool> {
        sqlx::query_scalar!(r#"SELECT length >= $3 AS "enough!" FROM Dicks d
                JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                    AND uid = $2"#,
                chat_id.value() as String, user_id as UserId, length as Bet)
            .fetch_optional(&self.pool)
            .map_ok(|opt| opt.unwrap_or(false))
            .await
            .context(format!("couldn't check the dick {chat_id}, {user_id} to have at least {length} cm"))
    }

    pub async fn move_length(&self, chat_id: &ChatIdPartiality, from: UserId, to: UserId, length: Bet) -> anyhow::Result<(GrowthResult, GrowthResult)> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;
        let winner_change = length.as_length_change_for_winner();
        let loser_change = length.as_length_change_for_loser();

        let mut tx = self.pool.begin().await?;
        let length_from = Self::move_length_for_one_user(&mut tx, internal_chat_id, from, loser_change).await?;
        let length_to = Self::move_length_for_one_user(&mut tx, internal_chat_id, to, winner_change).await?;
        tx.commit().await?;

        let pos_from = self.get_position_in_top(internal_chat_id, from).await?;
        let pos_to = self.get_position_in_top(internal_chat_id, to).await?;
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

    async fn move_length_for_one_user(tx: &mut Transaction<'_, Postgres>, chat_id_internal: InternalChatId, user_id: UserId, change: LengthChange) -> anyhow::Result<Length> {
        sqlx::query_scalar!("UPDATE Dicks SET length = (length + $3), bonus_attempts = (bonus_attempts + 1) WHERE chat_id = $1 AND uid = $2 RETURNING length",
                    chat_id_internal as InternalChatId, user_id as UserId, change.value() as i64)
            .fetch_one(&mut **tx)
            .await
            .map(Length::new)
            .context(format!("couldn't update the length by {change} for {chat_id_internal}, {user_id}"))
    }

    async fn get_position_in_top(&self, chat_id_internal: InternalChatId, uid: UserId) -> anyhow::Result<Option<Position>> {
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
                chat_id_internal as InternalChatId, uid as UserId)
            .fetch_one(&self.pool)
            .await
            .map(|pos| Some(Position::new(pos as u64)))
            .context(format!("couldn't get the top for {chat_id_internal} and {uid}"))
    }
    
    pub async fn grow_no_attempts_check(&self, chat_id: &ChatIdKind, user_id: UserId, change: LengthChange) -> anyhow::Result<GrowthResult> {
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;

        let new_length = Self::grow_no_attempts_check_internal(&self.pool, chat_internal_id, user_id, change).await?
            .ok_or(anyhow!("couldn't find a dick of ({chat_id}, {user_id}) for some reason"))?;
        let pos_in_top = self.get_position_in_top(chat_internal_id, user_id).await?;
        
        Ok(GrowthResult { new_length, pos_in_top })
    }

    pub(super) async fn grow_no_attempts_check_internal<'c, E>(executor: E, chat_id_internal: InternalChatId, user_id: UserId, bonus: LengthChange) -> anyhow::Result<Option<Length>>
    where E: Executor<'c, Database = Postgres>,
    {
        sqlx::query_scalar!(
            "UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $3)
                WHERE chat_id = $1 AND uid = $2
                RETURNING length",
                chat_id_internal as InternalChatId, user_id as UserId, bonus.value() as i64)
            .fetch_optional(executor)
            .await
            .map(|maybe_length| maybe_length.map(Length::new))
            .context(format!("couldn't grow the dick without attempts check for {chat_id_internal} and {user_id} by {bonus}"))
    }

    async fn insert_to_dod_table(tx: &mut Transaction<'_, Postgres>, chat_id_internal: InternalChatId, user_id: UserId) -> anyhow::Result<()> {
        sqlx::query!("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)",
                chat_id_internal as InternalChatId, user_id as UserId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't insert to DOD table for {chat_id_internal} and {user_id}"))?;
        Ok(())
    }
}
