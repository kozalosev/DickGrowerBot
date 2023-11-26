use sqlx::{Postgres, Transaction};
use teloxide::types::UserId;
use crate::{repo, repository};
use super::{ChatIdKind, ChatIdPartiality};

#[derive(sqlx::FromRow, Debug, sqlx::Type)]
pub struct Dick {
    pub length: i32,
    pub owner_name: String,
    pub grown_at: chrono::DateTime<chrono::Utc>,
    pub position: Option<i64>,
}

pub struct GrowthResult {
    pub new_length: i32,
    pub pos_in_top: u64,
}

repository!(Dicks,
    pub async fn create_or_grow(&self, uid: UserId, chat_id: &ChatIdPartiality, increment: i32) -> anyhow::Result<GrowthResult> {
        let uid = uid.0 as i64;
        let mut tx = self.pool.begin().await?;
        let internal_chat_id = repo::Chats::upsert_chat(&mut tx, chat_id).await?;
        let new_length = sqlx::query_scalar!(
            "INSERT INTO dicks(uid, chat_id, length, updated_at) VALUES ($1, $2, $3, current_timestamp)
                ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $3), updated_at = current_timestamp
                RETURNING length",
                uid, internal_chat_id, increment)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await? as u64;
        Ok(GrowthResult { new_length, pos_in_top })
    }
,
    pub async fn get_top(&self, chat_id: &ChatIdKind, offset: u32, limit: u32) -> anyhow::Result<Vec<Dick>, sqlx::Error> {
        sqlx::query_as!(Dick,
            r#"SELECT length, name as owner_name, updated_at as grown_at,
                    ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                FROM dicks d
                JOIN users using (uid)
                JOIN chats c ON c.id = d.chat_id
                WHERE c.chat_id = $1::bigint OR c.chat_instance = $1::text
                OFFSET $2 LIMIT $3"#,
                chat_id.value() as String, offset as i32, limit as i32)
            .fetch_all(&self.pool)
            .await
    }
,
    pub async fn set_dod_winner(&self, chat_id: &ChatIdPartiality, user_id: UserId, bonus: u32) -> anyhow::Result<Option<GrowthResult>> {
        let uid = user_id.0 as i64;
        let mut tx = self.pool.begin().await?;
        let internal_chat_id = repo::Chats::upsert_chat(&mut tx, chat_id).await?;
        let new_length = match Self::grow_dods_dick(&mut tx, internal_chat_id, uid, bonus as i32).await? {
            Some(length) => length,
            None => return Ok(None)
        };
        Self::insert_to_dod_table(&mut tx, internal_chat_id, uid).await?;
        tx.commit().await?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await? as u64;
        Ok(Some(GrowthResult { new_length, pos_in_top }))
    }
,
    async fn get_position_in_top(&self, chat_id_internal: i64, uid: i64) -> anyhow::Result<i64> {
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
            .map_err(|e| e.into())
    }
,
    async fn grow_dods_dick(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: i64, bonus: i32) -> anyhow::Result<Option<i32>> {
        sqlx::query_scalar!(
            "UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $3)
                WHERE chat_id = $1 AND uid = $2
                RETURNING length",
                chat_id_internal, user_id, bonus)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.into())
    }
,
    async fn insert_to_dod_table(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: i64) -> anyhow::Result<()> {
        sqlx::query!("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)",
                chat_id_internal, user_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
