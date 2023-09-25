use sqlx::{Postgres, Row, Transaction};
use teloxide::types::{ChatId, UserId};
use crate::repository;

#[derive(sqlx::FromRow, Debug)]
pub struct Dick {
    pub length: i32,
    pub owner_name: String,
}

#[derive(Copy, Clone)]
pub struct UID(pub i64);

repository!(Dicks,
    pub async fn create_or_grow(&self, uid: UserId, chat_id: ChatId, increment: i32) -> anyhow::Result<i32> {
        let uid: i64 = uid.0.try_into()?;
        sqlx::query("INSERT INTO dicks(uid, chat_id, length) VALUES ($1, $2, $3) ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $3) RETURNING length")
            .bind(uid)
            .bind(chat_id.0)
            .bind(increment)
            .fetch_one(&self.pool)
            .await?
            .try_get("length")
            .map_err(|e| e.into())
    }
,
    pub async fn get_top(&self, chat_id: ChatId) -> anyhow::Result<Vec<Dick>, sqlx::Error> {
        sqlx::query_as::<_, Dick>("SELECT length, name as owner_name FROM dicks d JOIN users u ON u.uid = d.uid WHERE chat_id = $1 ORDER BY length DESC LIMIT 10")
            .bind(chat_id.0)
            .fetch_all(&self.pool)
            .await
    }
,
    pub async fn set_dod_winner(&self, chat_id: ChatId, user_id: UID, bonus: u32) -> anyhow::Result<i32> {
        let mut tx = self.pool.begin().await?;
        let new_length = Self::grow_dods_dick(&mut tx, chat_id, user_id, bonus.try_into()?).await?;
        Self::insert_to_dod_table(&mut tx, chat_id, user_id).await?;
        tx.commit().await?;
        Ok(new_length)
    }
,
    async fn grow_dods_dick(tx: &mut Transaction<'_, Postgres>, chat_id: ChatId, user_id: UID, bonus: i32) -> anyhow::Result<i32> {
        sqlx::query("UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $3) WHERE chat_id = $1 AND uid = $2 RETURNING length")
            .bind(chat_id.0)
            .bind(user_id.0)
            .bind(bonus)
            .fetch_one(&mut **tx)
            .await?
            .try_get("length")
            .map_err(|e| e.into())
    }
,
    async fn insert_to_dod_table(tx: &mut Transaction<'_, Postgres>, chat_id: ChatId, user_id: UID) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)")
            .bind(chat_id.0)
            .bind(user_id.0)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
