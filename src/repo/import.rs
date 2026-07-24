use autometrics::autometrics;
use anyhow::Context;
use sqlx::{Postgres, Transaction};
use teloxide::types::ChatId;
use crate::domain::objects::ExternalUser;
use crate::domain::primitives::{Length, UserId};
use crate::repository;

repository!(Import,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = chat_id.0))]
    pub async fn get_imported_users(&self, chat_id: ChatId) -> anyhow::Result<Vec<ExternalUser>> {
        sqlx::query_as!(ExternalUser,
                r#"SELECT uid AS "uid: UserId", original_length AS "length: Length" FROM Imports WHERE chat_id = $1"#,
                chat_id.0 as i64)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't get imported users of {chat_id}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = chat_id.0))]
    pub async fn import(&self, chat_id: ChatId, users: &[ExternalUser]) -> anyhow::Result<()> {
        let chat_id = chat_id.0;
        let mut tx = self.pool.begin().await?;
        let uids = Self::insert_into_imports_table(&mut tx, chat_id, users).await?;
        Self::update_dicks(&mut tx, chat_id, uids).await?;
        tx.commit().await?;
        Ok(())
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id))]
    async fn insert_into_imports_table(
        tx: &mut Transaction<'_, Postgres>,
        chat_id: i64,
        users: &[ExternalUser],
    ) -> anyhow::Result<Vec<UserId>> {
        let (uids, lengths): (Vec<UserId>, Vec<Length>) = users.iter()
            .map(|user| (user.uid, user.length))
            .unzip();
        sqlx::query!("INSERT INTO Imports (chat_id, uid, original_length) SELECT $1, * FROM UNNEST($2::bigint[], $3::bigint[])",
                chat_id, &uids as &[UserId], &lengths as &[Length])
            .execute(&mut **tx)
            .await
            .context(format!("couldn't insert into imports table with chat_id = {chat_id} and users = {users:?}"))?;
        Ok(uids)
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id))]
    async fn update_dicks(tx: &mut Transaction<'_, Postgres>, chat_id: i64, uids: Vec<UserId>) -> anyhow::Result<()> {
        sqlx::query!("WITH original AS (SELECT c.id as chat_id, uid, original_length
                        FROM Imports JOIN Chats c USING (chat_id)
                        WHERE chat_id = $1 AND uid = ANY($2))
                            UPDATE Dicks d SET length = (length + original_length), bonus_attempts = (bonus_attempts + 1)
                            FROM original o WHERE d.chat_id = o.chat_id AND d.uid = o.uid",
                chat_id, &uids as &[UserId])
            .execute(&mut **tx)
            .await
            .context(format!("couldn't update dicks while importing in the chat with id = {chat_id}: {uids:?}"))?;
        Ok(())
    }
);
