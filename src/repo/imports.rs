use std::num::TryFromIntError;
use sqlx::{Postgres, Transaction};
use teloxide::types::{ChatId, UserId};
use crate::repository;

pub struct ExternalUser {
    uid: i64,
    length: i32
}

impl ExternalUser {
    pub fn new(uid: UserId, length: u32) -> Result<Self, TryFromIntError> {
        Ok(Self {
            uid: uid.0.try_into()?,
            length: length.try_into()?
        })
    }
}

repository!(Imports,
    pub async fn were_dicks_already_imported(&self, chat_id: ChatId) -> anyhow::Result<bool> {
        sqlx::query_scalar("SELECT count(*) > 0 AS exists FROM Imports WHERE chat_id = $1")
            .bind(chat_id.0)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    pub async fn import(&self, chat_id: ChatId, users: &Vec<ExternalUser>) -> anyhow::Result<()> {
        let chat_id: i64 = chat_id.0.try_into()?;
        let mut tx = self.pool.begin().await?;

        let uids = Self::insert_to_imports_table(&mut tx, chat_id, &users).await?;
        Self::update_dicks(&mut tx, chat_id, uids).await?;

        tx.commit().await?;
        Ok(())
    }
,
    async fn insert_to_imports_table(tx: &mut Transaction<'_, Postgres>, chat_id: i64, users: &Vec<ExternalUser>) -> anyhow::Result<Vec<i64>> {
        let (uids, lengths): (Vec<i64>, Vec<i32>) = users.iter()
            .map(|user| (user.uid, user.length))
            .unzip();
        sqlx::query("INSERT INTO Imports (chat_id, uid, original_length) SELECT $1, * FROM UNNEST($2::bigint[], $3::int[])")
            .bind(chat_id)
            .bind(&uids)
            .bind(lengths)
            .execute(&mut **tx)
            .await?;
        Ok(uids)
    }
,
    async fn update_dicks(tx: &mut Transaction<'_, Postgres>, chat_id: i64, uids: Vec<i64>) -> anyhow::Result<()> {
        sqlx::query("WITH orig AS (SELECT * FROM Imports WHERE chat_id = $1 AND uid = ANY($2))
                        UPDATE Dicks SET length = (length + orig.original_length)
                                     WHERE chat_id = orig.chat_id AND uid = orig.uid")
            .bind(chat_id)
            .bind(uids)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }
);
