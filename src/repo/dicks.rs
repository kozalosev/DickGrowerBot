use sqlx::{Postgres, Row, Transaction};
use teloxide::types::UserId;
use crate::repository;
use super::{ChatIdKind, UID};

#[derive(sqlx::FromRow, Debug)]
pub struct Dick {
    pub length: i32,
    pub owner_name: String,
    pub grown_at: chrono::DateTime<chrono::Utc>,
}

pub struct GrowthResult {
    pub new_length: i32,
    pub pos_in_top: u64,
}

repository!(Dicks,
    pub async fn create_or_grow(&self, uid: UserId, chat_id: &ChatIdKind, increment: i32) -> anyhow::Result<GrowthResult> {
        let uid = uid.0 as i64;
        let mut tx = self.pool.begin().await?;
        let internal_chat_id = Self::upsert_chat(&mut tx, chat_id).await?;
        let new_length = sqlx::query(
            "INSERT INTO dicks(uid, chat_id, length, updated_at) VALUES ($1, $2, $5, current_timestamp)
                ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $5), updated_at = current_timestamp
                RETURNING length")
            .bind(uid)
            .bind(internal_chat_id)
            .bind(chat_id.kind())
            .bind(chat_id.value())
            .bind(increment)
            .fetch_one(&mut *tx)
            .await?
            .try_get("length")?;
        tx.commit().await?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await? as u64;
        Ok(GrowthResult { new_length, pos_in_top })
    }
,
    pub async fn get_top(&self, chat_id: &ChatIdKind, limit: u32) -> anyhow::Result<Vec<Dick>, sqlx::Error> {
        sqlx::query_as::<_, Dick>(
            "SELECT length, name as owner_name, updated_at as grown_at FROM dicks d
                JOIN users USING (uid)
                JOIN chats c ON c.id = d.chat_id
                WHERE c.type = $1::chat_id_type AND (c.chat_id = $2::bigint OR c.chat_instance = $2::text)
                ORDER BY length DESC, updated_at DESC, name
                LIMIT $3")
            .bind(chat_id.kind())
            .bind(chat_id.value())
            .bind(limit as i32)
            .fetch_all(&self.pool)
            .await
    }
,
    pub async fn set_dod_winner(&self, chat_id: &ChatIdKind, user_id: UID, bonus: u32) -> anyhow::Result<GrowthResult> {
        let mut tx = self.pool.begin().await?;
        let internal_chat_id = Self::upsert_chat(&mut tx, &chat_id).await?;
        let new_length = Self::grow_dods_dick(&mut tx, internal_chat_id, user_id, bonus as i32).await?;
        Self::insert_to_dod_table(&mut tx, internal_chat_id, user_id).await?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, user_id.0).await? as u64;
        tx.commit().await?;
        Ok(GrowthResult { new_length, pos_in_top })
    }
,
    async fn upsert_chat(tx: &mut Transaction<'_, Postgres>, chat_id: &ChatIdKind) -> anyhow::Result<i64> {
        let (id, instance) = match &chat_id {
            ChatIdKind::ID(x) => (Some(x.0), None),
            ChatIdKind::Instance(x) => (None, Some(x))
        };
        sqlx::query_scalar("
            WITH new_chat AS (
                INSERT INTO Chats (type, chat_id, chat_instance)
                VALUES ($1::chat_id_type, $2, $3)
                ON CONFLICT DO NOTHING
                RETURNING id
            ) SELECT coalesce(
                (SELECT id FROM new_chat),
                (SELECT id FROM Chats WHERE type = $1::chat_id_type AND (chat_id = $2 OR chat_instance = $3))
            )")
            .bind(chat_id.kind())
            .bind(id)
            .bind(instance)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| e.into())
    }
,
    async fn get_position_in_top(&self, chat_id_internal: i64, uid: i64) -> anyhow::Result<i64> {
        sqlx::query("SELECT position FROM (
                        SELECT uid, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                        FROM dicks
                        JOIN users using (uid)
                        WHERE chat_id = $1
                    ) AS _
                    WHERE uid = $2")
            .bind(chat_id_internal)
            .bind(uid)
            .fetch_one(&self.pool)
            .await?
            .try_get::<i64, _>("position")
            .map_err(|e| e.into())
    }
,
    async fn grow_dods_dick(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: UID, bonus: i32) -> anyhow::Result<i32> {
        sqlx::query("UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $3)
                WHERE chat_id = $1 AND uid = $2
                RETURNING length")
            .bind(chat_id_internal)
            .bind(user_id.0)
            .bind(bonus)
            .fetch_one(&mut **tx)
            .await?
            .try_get("length")
            .map_err(|e| e.into())
    }
,
    async fn insert_to_dod_table(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: UID) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)")
            .bind(chat_id_internal)
            .bind(user_id.0)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
