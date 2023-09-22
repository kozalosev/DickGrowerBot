use sqlx::{Postgres, Row};
use teloxide::types::{ChatId, UserId};
use crate::repository;

#[derive(sqlx::FromRow, Debug)]
pub struct Dick {
    pub length: i32,
    pub owner_name: String,
}

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
);
