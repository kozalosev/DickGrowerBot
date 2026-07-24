use autometrics::autometrics;
use anyhow::Context;
use crate::domain::objects::User;
use crate::domain::primitives::{Ratio, UserId, Username};
use crate::repo::ChatIdKind;
use crate::repository;

repository!(Users,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(user_id = user_id.value(), name = %name))]
    pub async fn create_or_update(&self, user_id: UserId, name: &str) -> anyhow::Result<User> {
        sqlx::query_as!(User,
            r#"INSERT INTO Users(uid, name) VALUES ($1, $2)
                ON CONFLICT (uid) DO UPDATE SET name = $2
                RETURNING uid AS "uid: UserId", name AS "name: Username", created_at"#,
                user_id as UserId, name)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't upsert a user with id = {user_id}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn get_chat_members(&self, chat_id: &ChatIdKind) -> anyhow::Result<Vec<User>> {
        sqlx::query_as!(User,
            r#"SELECT u.uid AS "uid: UserId", name AS "name: Username", created_at FROM Users u
                JOIN Dicks d USING (uid)
                JOIN Chats c ON d.chat_id = c.id
                WHERE c.chat_id = $1::bigint OR c.chat_instance = $1::text"#,
                chat_id.value() as String)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't get users of the chat with id = {chat_id}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn get_random_active_member(&self, chat_id: &ChatIdKind) -> anyhow::Result<Option<User>> {
        sqlx::query_as!(User,
            r#"SELECT u.uid AS "uid: UserId", name AS "name: Username", u.created_at FROM Users u
                JOIN Dicks d USING (uid)
                JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                    AND updated_at > current_timestamp - interval '1 week'
                ORDER BY random() LIMIT 1"#,
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get a random active user of the chat with id = {chat_id}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, rich_exclusion_ratio = rich_exclusion_ratio.value()))]
    pub async fn get_random_active_poor_member(
        &self,
        chat_id: &ChatIdKind,
        rich_exclusion_ratio: Ratio,
    ) -> anyhow::Result<Option<User>> {
        let wealth_borderline = (Ratio::literal(1.0) - rich_exclusion_ratio)?;
        sqlx::query_as!(User,
            r#"WITH ranked_users AS (
                SELECT u.uid, name, u.created_at, PERCENT_RANK() OVER (ORDER BY length) AS percentile_rank
                    FROM Users u
                    JOIN Dicks d USING (uid)
                    JOIN Chats c ON d.chat_id = c.id
                    WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                        AND updated_at > current_timestamp - interval '1 week'
            )
            SELECT uid AS "uid: UserId", name AS "name: Username", created_at
            FROM ranked_users
            WHERE percentile_rank <= $2
            ORDER BY random() LIMIT 1"#,
                chat_id.value() as String, wealth_borderline as Ratio)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get a random active poor user of the chat with id = {chat_id}"))
    }
,
    // Weighted-random selection where a member's weight is a sigmoid over the z-score of their
    // length: the weight adapts to the actual spread of lengths in the chat instead of a fixed
    // scale, so a poorer member is favored while everyone keeps a realistic chance (see #60).
    // When every length is equal (STDDEV_POP = 0), the z-score is coalesced to 0, which yields
    // an equal weight of 0.5 for all the members.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn get_random_active_member_with_poor_in_priority(
        &self,
        chat_id: &ChatIdKind,
    ) -> anyhow::Result<Option<User>> {
        sqlx::query_as!(User,
            r#"WITH user_weights AS (
                SELECT u.uid, u.name, u.created_at, d.length,
                       1.0 / (1.0 + EXP(COALESCE(
                           (d.length - AVG(d.length) OVER ()) / NULLIF(STDDEV_POP(d.length) OVER (), 0),
                           0))) AS weight
                FROM Users u
                  JOIN Dicks d USING (uid)
                  JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                  AND d.updated_at > current_timestamp - interval '1 week'
            ),
                 cumulative_weights AS (
                     SELECT uid, name, created_at, weight,
                            SUM(weight) OVER (ORDER BY uid) AS cumulative_weight, -- Cumulative weight
                            SUM(weight) OVER () AS total_weight
                     FROM user_weights
                 ),
                 random_value AS (
                     SELECT RANDOM() * (SELECT total_weight FROM cumulative_weights LIMIT 1) AS rand_value  -- Generate one random value
                 )
            SELECT uid AS "uid: UserId", name AS "name: Username", created_at
            FROM cumulative_weights, random_value
            WHERE cumulative_weight >= random_value.rand_value
            ORDER BY cumulative_weight
            LIMIT 1;  -- Select the first user whose cumulative weight exceeds the random value"#,
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get a random active user of the chat with id = {chat_id}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(user_id = user_id.value()))]
    pub async fn get(&self, user_id: UserId) -> anyhow::Result<Option<User>> {
        sqlx::query_as!(User, r#"SELECT uid AS "uid: UserId", name AS "name: Username", created_at FROM Users WHERE uid = $1"#, user_id as UserId)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get a user with id = {user_id}"))
    }
,
    #[cfg(test)]
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn get_all(&self) -> anyhow::Result<Vec<User>> {
        sqlx::query_as!(User, r#"SELECT uid AS "uid: UserId", name AS "name: Username", created_at FROM Users"#)
            .fetch_all(&self.pool)
            .await
            .map_err(Into::into)
    }
);
