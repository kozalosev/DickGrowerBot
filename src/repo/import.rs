use anyhow::Context;
use teloxide::types::ChatId;
use crate::domain::objects::ExternalUser;
use crate::domain::primitives::{Length, UserId};
use crate::repository;

repository!(Import,
    pub async fn get_imported_users(&self, chat_id: ChatId) -> anyhow::Result<Vec<ExternalUser>> {
        sqlx::query_as!(ExternalUser,
                r#"SELECT uid AS "uid: UserId", original_length AS "length: Length" FROM Imports WHERE chat_id = $1"#,
                chat_id.0 as i64)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't get imported users of {chat_id}"))
    }
,
    pub async fn import(&self, chat_id: ChatId, users: &[ExternalUser]) -> anyhow::Result<()> {
        let (uids, lengths): (Vec<UserId>, Vec<Length>) = users.iter()
            .map(|user| (user.uid, user.length))
            .unzip();
        sqlx::query!("WITH inserted AS (
                        INSERT INTO Imports (chat_id, uid, original_length)
                        SELECT $1, * FROM UNNEST($2::bigint[], $3::bigint[])
                        RETURNING chat_id, uid, original_length
                    )
                    UPDATE Dicks d SET length = (d.length + i.original_length), bonus_attempts = (d.bonus_attempts + 1)
                    FROM inserted i JOIN Chats c ON c.chat_id = i.chat_id
                    WHERE d.chat_id = c.id AND d.uid = i.uid",
                chat_id.0, &uids as &[UserId], &lengths as &[Length])
            .execute(&self.pool)
            .await
            .context(format!("couldn't import {users:?} into the chat with id = {chat_id}"))?;
        Ok(())
    }
);
