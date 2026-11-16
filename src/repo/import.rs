use autometrics::autometrics;
use anyhow::Context;
use teloxide::types::ChatId;
use crate::domain::objects::ExternalUser;
use crate::domain::primitives::{Count, Length, UserId};
use crate::errors::details::{DetailedContext, Details};
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
            .context("couldn't get the imported users of the chat")
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = chat_id.0))]
    pub async fn import(&self, chat_id: ChatId, users: &[ExternalUser]) -> anyhow::Result<()> {
        let (uids, lengths): (Vec<UserId>, Vec<Length>) = users.iter()
            .map(|user| (user.uid, user.length))
            .unzip();
        sqlx::query!("WITH inserted AS (
                        INSERT INTO Imports (chat_id, uid, original_length)
                        SELECT $1, * FROM UNNEST($2::bigint[], $3::bigint[])
                        RETURNING chat_id, uid, original_length
                    )
                    UPDATE Dicks d SET length = (d.length + i.original_length)
                    FROM inserted i JOIN Chats c ON c.chat_id = i.chat_id
                    WHERE d.chat_id = c.id AND d.uid = i.uid",
                chat_id.0, &uids as &[UserId], &lengths as &[Length])
            .execute(&self.pool)
            .await
            .detailed_context(Details::new("couldn't import the users")
                .count(Count::<ExternalUser>::new(u64::try_from(users.len())?)))?;
        Ok(())
    }
);
