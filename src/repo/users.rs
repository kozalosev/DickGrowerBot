use sqlx::Postgres;
use teloxide::types::UserId;
use crate::repository;

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
);
