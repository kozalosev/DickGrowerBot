use teloxide::types::ChatId;
use crate::repository;

repository!(Imports,
    pub async fn were_dicks_already_imported(&self, chat_id: ChatId) -> anyhow::Result<bool> {
        sqlx::query_as::<_, bool>("SELECT count(*) > 0 AS exists FROM Imports WHERE chat_id = $1")
            .bind(chat_id.0)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn notify(&self, chat_id: ChatId) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO Imports (chat_id) VALUES ($1)")
            .bind(chat_id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
);
