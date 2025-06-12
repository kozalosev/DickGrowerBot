use std::fmt::Formatter;
use anyhow::{bail, Context};
use sqlx::{Postgres, Transaction};
use teloxide::types::ChatId;
use crate::domain::primitives::InternalChatId;
use super::{ChatIdFull, ChatIdKind, ChatIdPartiality, ChatIdSource, ensure_only_one_row_updated};
use crate::repository;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Chat {
    pub internal_id: i64,
    pub chat_id: Option<i64>,
    pub chat_instance: Option<String>,
}

#[derive(Debug, derive_more::Error, derive_more::Display)]
pub struct NoChatIdError(#[error(not(source))] i64);

/// SearchError is used when Option<T> may be returned theoretically but shouldn't in practice.
#[derive(Debug, derive_more::Error, derive_more::Display)]
pub enum SearchError<KEY> {
    NotFound(#[error(not(source))] KEY),
    Internal(anyhow::Error)
}

impl TryInto<ChatIdPartiality> for Chat {
    type Error = NoChatIdError;

    fn try_into(self) -> Result<ChatIdPartiality, Self::Error> {
        match (self.chat_id, self.chat_instance) {
            (Some(id), Some(instance)) => Ok(ChatIdPartiality::Both(ChatIdFull { id: ChatId(id), instance }, ChatIdSource::Database)),
            (Some(id), None) => Ok(ChatIdPartiality::Specific(ChatIdKind::ID(ChatId(id)))),
            (None, Some(instance)) => Ok(ChatIdPartiality::Specific(ChatIdKind::Instance(instance))),
            (None, None) => Err(NoChatIdError(self.internal_id))
        }
    }
}

repository!(Chats, with_feature_toggles,
    pub async fn get_chat(&self, chat_id: ChatIdKind) -> anyhow::Result<Option<Chat>> {
        sqlx::query_as!(Chat, "SELECT id as internal_id, chat_id, chat_instance FROM Chats
                WHERE chat_id = $1::bigint OR chat_instance = $1::text",
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get the information about the chat with id = {chat_id}"))
    }
,
    pub async fn get_internal_id(&self, chat_id: &ChatIdKind) -> Result<i64, SearchError<ChatIdKind>> {
        self.get_chat(chat_id.clone()).await
            .map_err(SearchError::Internal)?
            .map(|chat| chat.internal_id)
            .ok_or(SearchError::NotFound(chat_id.clone()))
    }
,
    pub async fn upsert_chat(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<InternalChatId> {
        let (id, instance) = match chat_id {
            ChatIdPartiality::Both(full, _) if self.features.chats_merging => (Some(full.id.0), Some(full.instance.to_owned())),
            ChatIdPartiality::Both(full, ChatIdSource::Database) => (Some(full.id.0), None),
            ChatIdPartiality::Both(full, ChatIdSource::InlineQuery) => (None, Some(full.instance.clone())),
            ChatIdPartiality::Specific(ChatIdKind::ID(id)) => (Some(id.0), None),
            ChatIdPartiality::Specific(ChatIdKind::Instance(instance)) => (None, Some(instance.to_owned())),
        };
        let mut tx = self.pool.begin().await?;
        let chats = sqlx::query_as!(Chat, "SELECT id as internal_id, chat_id, chat_instance FROM Chats
                WHERE chat_id = $1 OR chat_instance = $2",
                id, instance)
            .fetch_all(&mut *tx)
            .await
            .context(format!("couldn't find the chat with id = {chat_id}"))?;
        let internal_id = match chats.len() {
            1 if chats[0].chat_id == id && chats[0].chat_instance == instance => Ok(chats[0].internal_id),
            1 => Self::update_chat(&mut tx, chats[0].internal_id, id, instance).await,
            0 => Self::create_chat(&mut tx, id, instance).await,
            2 => Self::merge_chats(&mut tx, [&chats[0], &chats[1]]).await,
            x => bail!("unexpected count of chats ({x}): {chats:?}"),
        }?;
        tx.commit().await?;
        Ok(internal_id)
    }
,
    async fn create_chat(tx: &mut Transaction<'_, Postgres>, chat_id: Option<i64>, chat_instance: Option<String>) -> anyhow::Result<i64> {
        log::info!("creating a chat with chat_id = {chat_id:?} and chat_instance = {chat_instance:?}");
        sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, $2) RETURNING id",
                chat_id, chat_instance)
            .fetch_one(&mut **tx)
            .await
            .context(format!("couldn't create a chat with chat_id = {chat_id:?} or chat_instance = {chat_instance:?}"))
    }
,
    async fn update_chat(tx: &mut Transaction<'_, Postgres>, internal_id: i64, chat_id: Option<i64>, chat_instance: Option<String>) -> anyhow::Result<i64> {
        log::debug!("updating the chat with id = {internal_id}, chat_id = {chat_id:?}, and chat_instance = {chat_instance:?}");
        sqlx::query!("UPDATE Chats SET chat_id = coalesce($2, chat_id), chat_instance = coalesce($3, chat_instance) WHERE id = $1",
                internal_id, chat_id, chat_instance)
            .execute(&mut **tx)
            .await
            .map(|_| internal_id)
            .context(format!("couldn't update the chat with id = {internal_id} to chat_id = {chat_id:?}, chat_instance = {chat_instance:?}))"))
    }
,
    async fn merge_chats(tx: &mut Transaction<'_, Postgres>, chats: [&Chat; 2]) -> anyhow::Result<i64> {
        let state = merge_chat_objects(&chats)?;
        let updated_dicks = sqlx::query!(
            "WITH sum_dicks AS (SELECT uid, sum(length) as length FROM Dicks WHERE chat_id IN ($1, $2) GROUP BY uid)
                    UPDATE Dicks d SET length = sum_dicks.length, bonus_attempts = (bonus_attempts + 1)
                    FROM sum_dicks WHERE chat_id = $1 AND d.uid = sum_dicks.uid",
                state.main.internal_id, state.deleted.0)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't update dicks while merging in the chats = {chats:?}"))?
            .rows_affected();
        let deleted_dicks = sqlx::query!("DELETE FROM Dicks WHERE chat_id = $1", state.deleted.0)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete dicks from the old chat with id = {}", state.deleted.0))?
            .rows_affected();
        log::info!("merging chats: {chats:?}, updated dicks: {updated_dicks}, deleted: {deleted_dicks}");

        sqlx::query!("DELETE FROM Chats WHERE id = $1 AND chat_instance = $2",
                state.deleted.0, state.deleted.1)
            .execute(&mut **tx)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't delete the old chat with id = {} and chat_instance = {}", state.deleted.0, state.deleted.1))?;
        sqlx::query!("UPDATE Chats SET chat_instance = $3 WHERE id = $1 AND chat_id = $2",
                state.main.internal_id, state.main.chat_id, state.main.chat_instance)
            .execute(&mut **tx)
            .await
            .map_err(Into::into)
            .and_then(ensure_only_one_row_updated)
            .context(format!("couldn't update chat_instance of the new chat with ids = {} and {} to {}", state.main.internal_id, state.main.chat_id, state.main.chat_instance))?;
        Ok(state.main.internal_id)
    }
);

struct MergedChat<'a> {
    internal_id: i64,
    chat_id: i64,
    chat_instance: &'a str
}

struct MergedChatState<'a> {
    main: MergedChat<'a>,
    deleted: (i64, &'a str)
}

#[derive(Debug, derive_more::Error)]
struct MergeChatsError([Chat; 2], String);

impl std::fmt::Display for MergeChatsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("MergeChatsError ({}): {:?}", self.1, self.0))
    }
}

impl MergeChatsError {
    fn new(chats: &[&Chat; 2], msg: &str) -> Self {
        Self(chats.map(Chat::to_owned), msg.to_owned())
    }
}

fn merge_chat_objects<'a>(chats: &'a [&Chat; 2]) -> Result<MergedChatState<'a>, MergeChatsError> {
    let chat_ids: Vec<(i64, i64)> = chats.iter()
        .filter_map(|c| c.chat_id.map(|id| (c.internal_id, id)))
        .collect();
    let chat_instances: Vec<(i64, &'a String)> = chats.iter()
        .filter_map(|c| c.chat_instance.as_ref().map(|inst| (c.internal_id, inst)))
        .collect();

    if chat_ids.len() != 1 || chat_instances.len() != 1 {
        Err(MergeChatsError::new(chats, "both chats contain the same identifiers"))
    } else if chat_ids[0].0 == chat_instances[0].0 {
        Err(MergeChatsError::new(chats, "both chats have the same internal id"))
    } else {
        Ok(MergedChatState::<'a> {
            main: MergedChat {
                internal_id: chat_ids[0].0,
                chat_id: chat_ids[0].1,
                chat_instance: chat_instances[0].1,
            },
            deleted: (chat_instances[0].0, chat_instances[0].1)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Chat, merge_chat_objects};

    #[test]
    fn merge_valid_chats() {
        let id = 123;
        let inst = "one".to_owned();
        let chat1 = Chat {
            internal_id: 1,
            chat_id: Some(id),
            chat_instance: None,
        };
        let chat2 = Chat {
            internal_id: 2,
            chat_id: None,
            chat_instance: Some(inst.clone()),
        };
        let chats = [&chat1, &chat2];
        let res = merge_chat_objects(&chats)
            .expect("merge_chat_objects failed");

        assert_eq!(res.main.internal_id, 1);
        assert_eq!(res.main.chat_id, id);
        assert_eq!(res.main.chat_instance, &inst);
        assert_eq!(res.deleted.0, 2);
        assert_eq!(res.deleted.1, &inst);
    }

    #[test]
    fn merge_both_filled_chats() {
        let id = 123;
        let inst = "one".to_owned();
        let chat1 = Chat {
            internal_id: 1,
            chat_id: Some(id),
            chat_instance: Some(inst.clone()),
        };
        let chat2 = Chat {
            internal_id: 2,
            chat_id: Some(id),
            chat_instance: Some(inst.clone()),
        };
        let chats = [&chat1, &chat2];
        let res = merge_chat_objects(&chats);

        assert!(res.is_err())
    }

    #[test]
    fn merge_chats_with_same_id() {
        let chat1 = Chat {
            internal_id: 1,
            chat_id: Some(123),
            chat_instance: None,
        };
        let chat2 = Chat {
            internal_id: 1,
            chat_id: None,
            chat_instance: Some("one".to_owned()),
        };
        let chats = [&chat1, &chat2];
        let res = merge_chat_objects(&chats);

        assert!(res.is_err())
    }
}
