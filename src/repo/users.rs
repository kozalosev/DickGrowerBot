use teloxide::types::{ChatId, UserId};
use crate::repository;

#[derive(sqlx::FromRow, Debug)]
pub struct User {
    pub uid: i64,
    pub name: String,
}

repository!(Users,
    pub async fn create_or_update(&self, uid: UserId, name: String) -> anyhow::Result<()> {
        let uid: i64 = uid.0.try_into()?;
        sqlx::query("INSERT INTO Users(uid, name) VALUES ($1, $2) ON CONFLICT (uid) DO UPDATE SET name = $2")
            .bind(uid)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
,
    pub async fn get_chat_members(&self, chat_id: ChatId) -> anyhow::Result<Vec<User>> {
        sqlx::query_as::<_, User>("SELECT u.uid, name FROM Users u JOIN Dicks d ON u.uid = d.uid WHERE chat_id = $1")
            .bind(chat_id.0)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get_random_active_member(&self, chat_id: ChatId) -> anyhow::Result<User> {
        let sql = "SELECT u.uid, name FROM Users u
            JOIN Dicks d ON u.uid = d.uid
            WHERE chat_id = $1 AND updated_at > current_timestamp - interval '1 week'
            ORDER BY random() LIMIT 1";
        sqlx::query_as::<_, User>(sql)
            .bind(chat_id.0)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.into())
    }
);
