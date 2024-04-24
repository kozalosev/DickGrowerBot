use sqlx::{Postgres, Transaction};
use teloxide::types::{ChatId, UserId};
use crate::repository;

#[derive(Eq, PartialEq, Debug, sqlx::FromRow)]
pub struct ExternalUser {
    pub uid: i64,
    pub length: i32
}

impl ExternalUser {
    pub fn new(uid: UserId, length: u32) -> Self {
        Self {
            uid: uid.0 as i64,
            length: length as i32
        }
    }
}

repository!(Import,
    pub async fn get_imported_users(&self, chat_id: ChatId) -> anyhow::Result<Vec<ExternalUser>> {
        sqlx::query_as!(ExternalUser,
                "SELECT uid, original_length AS length FROM Imports WHERE chat_id = $1",
                chat_id.0 as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn import(&self, chat_id: ChatId, users: &[ExternalUser]) -> anyhow::Result<()> {
        let chat_id = chat_id.0;
        let mut tx = self.pool.begin().await?;
        let uids = Self::insert_into_imports_table(&mut tx, chat_id, users).await?;
        Self::update_dicks(&mut tx, chat_id, uids).await?;
        tx.commit().await?;
        Ok(())
    }
,
    async fn insert_into_imports_table(tx: &mut Transaction<'_, Postgres>, chat_id: i64, users: &[ExternalUser]) -> anyhow::Result<Vec<i64>> {
        let (uids, lengths): (Vec<i64>, Vec<i32>) = users.iter()
            .map(|user| (user.uid, user.length))
            .unzip();
        sqlx::query!("INSERT INTO Imports (chat_id, uid, original_length) SELECT $1, * FROM UNNEST($2::bigint[], $3::int[])",
                chat_id, &uids, &lengths)
            .execute(&mut **tx)
            .await?;
        Ok(uids)
    }
,
    async fn update_dicks(tx: &mut Transaction<'_, Postgres>, chat_id: i64, uids: Vec<i64>) -> anyhow::Result<()> {
        sqlx::query!("WITH original AS (SELECT c.id as chat_id, uid, original_length
                        FROM Imports JOIN Chats c USING (chat_id)
                        WHERE chat_id = $1 AND uid = ANY($2))
                            UPDATE Dicks d SET length = (length + original_length), bonus_attempts = (bonus_attempts + 1)
                            FROM original o WHERE d.chat_id = o.chat_id AND d.uid = o.uid",
                chat_id, &uids)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
