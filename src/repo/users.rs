use chrono::{DateTime, Utc};
use teloxide::types::UserId;

use crate::domain::{Ratio, Username};
use crate::repo::ChatIdKind;
use crate::repository;

#[derive(sqlx::FromRow, Debug)]
pub struct User {
    pub uid: i64,
    pub name: Username,
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
        sqlx::query_as!(User,
            "SELECT u.uid, name, created_at FROM Users u
                JOIN Dicks d USING (uid)
                JOIN Chats c ON d.chat_id = c.id
                WHERE c.chat_id = $1::bigint OR c.chat_instance = $1::text",
                chat_id.value() as String)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get_random_active_member(&self, chat_id: &ChatIdKind) -> anyhow::Result<Option<User>> {
        sqlx::query_as!(User,
            "SELECT u.uid, name, u.created_at FROM Users u
                JOIN Dicks d USING (uid)
                JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                    AND updated_at > current_timestamp - interval '1 week'
                ORDER BY random() LIMIT 1",
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get_random_active_poor_member(&self, chat_id: &ChatIdKind, rich_exclusion_ratio: Ratio) -> anyhow::Result<Option<User>> {
        sqlx::query_as!(User,
            "WITH ranked_users AS (
                SELECT u.uid, name, u.created_at, PERCENT_RANK() OVER (ORDER BY length) AS percentile_rank
                    FROM Users u
                    JOIN Dicks d USING (uid)
                    JOIN Chats c ON d.chat_id = c.id
                    WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                        AND updated_at > current_timestamp - interval '1 week'
            )
            SELECT uid, name, created_at
            FROM ranked_users
            WHERE percentile_rank <= $2
            ORDER BY random() LIMIT 1",
                chat_id.value() as String, 1.0 - rich_exclusion_ratio.to_value())
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get_random_active_member_with_poor_in_priority(&self, chat_id: &ChatIdKind) -> anyhow::Result<Option<User>> {
        sqlx::query_as!(User,
            "WITH user_weights AS (
                SELECT u.uid, u.name, u.created_at, d.length,
                       CASE
                           WHEN d.length = 0 THEN 1.0
                           ELSE 1.0 / d.length
                       END AS weight,
                       SUM(CASE
                           WHEN d.length = 0 THEN 1.0
                           ELSE 1.0 / d.length 
                       END) OVER () AS total_weight
                FROM Users u
                JOIN Dicks d USING (uid)
                JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                      AND d.updated_at > current_timestamp - interval '1 week'
            ),
            weighted_users AS (
                SELECT uid, name, created_at, weight,
                       total_weight,
                       (RANDOM() * total_weight) AS rand_weight
                FROM user_weights
            )
            SELECT uid, name, created_at
            FROM weighted_users
            WHERE rand_weight < weight
            ORDER BY random()
            LIMIT 1",
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn get(&self, user_id: UserId) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as!(User, "SELECT uid, name, created_at FROM Users WHERE uid = $1",
                user_id.0 as i64)
            .fetch_optional(&self.pool)
            .await
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
