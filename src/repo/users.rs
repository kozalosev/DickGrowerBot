use chrono::{DateTime, Utc};
use teloxide::types::UserId;
use crate::repo::{ChatIdKind, ChatIdType};
use crate::repository;

#[derive(sqlx::FromRow, Debug)]
pub struct User {
    pub uid: i64,
    pub name: String,
    pub created_at: DateTime<Utc>
}

repository!(Users,
    pub async fn create_or_update(&self, user_id: UserId, name: &str) -> anyhow::Result<User> {
        let uid = user_id.0 as i64;
        sqlx::query_as!(User,
            "INSERT INTO Users(uid, name) VALUES ($1, $2)
                ON CONFLICT (uid) DO UPDATE SET name = $2
                RETURNING uid, name, created_at",
                uid, name)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get_chat_members(&self, chat_id: &ChatIdKind) -> anyhow::Result<Vec<User>> {
        let chat_id_type: ChatIdType = chat_id.into();
        sqlx::query_as!(User,
            "SELECT u.uid, name, created_at FROM Users u
                JOIN Dicks d ON u.uid = d.uid
                JOIN Chats c ON d.chat_id = c.id
                WHERE c.type = $1 AND (c.chat_id = $2::bigint OR c.chat_instance = $2::text)",
                chat_id_type as ChatIdType, chat_id.value() as String)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get_random_active_member(&self, chat_id: &ChatIdKind) -> anyhow::Result<Option<User>> {
        let chat_id_type: ChatIdType = chat_id.into();
        sqlx::query_as!(User,
            "SELECT u.uid, name, u.created_at FROM Users u
                JOIN Dicks d ON u.uid = d.uid
                JOIN Chats c ON d.chat_id = c.id
                WHERE c.type = $1::chat_id_type AND (c.chat_id = $2::bigint OR c.chat_instance = $2::text)
                    AND updated_at > current_timestamp - interval '1 week'
                ORDER BY random() LIMIT 1",
                chat_id_type as ChatIdType, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    #[cfg(test)]
    pub async fn get_all(&self) -> anyhow::Result<Vec<User>> {
        sqlx::query_as!(User, "SELECT * FROM Users")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.into())
    }
);
