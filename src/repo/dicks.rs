use std::fmt::Formatter;
use anyhow::anyhow;
use sqlx::{Postgres, Transaction};
use sqlx::postgres::PgQueryResult;
use teloxide::types::{ChatId, UserId};
use crate::repository;
use super::{ChatIdFull, ChatIdKind, ChatIdPartiality};

#[derive(sqlx::FromRow, Debug, sqlx::Type)]
pub struct Dick {
    pub length: i32,
    pub owner_name: String,
    pub grown_at: chrono::DateTime<chrono::Utc>,
    pub position: Option<i64>,
}

pub struct GrowthResult {
    pub new_length: i32,
    pub pos_in_top: u64,
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Chat {
    pub internal_id: i64,
    pub chat_id: Option<i64>,
    pub chat_instance: Option<String>,
}

#[derive(Debug, derive_more::Error, derive_more::Display)]
pub struct NoChatIdError(#[error(not(source))] i64);

impl TryInto<ChatIdPartiality> for Chat {
    type Error = NoChatIdError;

    fn try_into(self) -> Result<ChatIdPartiality, Self::Error> {
        match (self.chat_id, self.chat_instance) {
            (Some(id), Some(instance)) => Ok(ChatIdPartiality::Both(ChatIdFull { id: ChatId(id), instance })),
            (Some(id), None) => Ok(ChatIdPartiality::Specific(ChatIdKind::ID(ChatId(id)))),
            (None, Some(instance)) => Ok(ChatIdPartiality::Specific(ChatIdKind::Instance(instance))),
            (None, None) => Err(NoChatIdError(self.internal_id))
        }
    }
}

repository!(Dicks,
    pub async fn create_or_grow(&self, uid: UserId, chat_id: &ChatIdPartiality, increment: i32) -> anyhow::Result<GrowthResult> {
        let uid = uid.0 as i64;
        let mut tx = self.pool.begin().await?;
        let internal_chat_id = Self::upsert_chat(&mut tx, chat_id).await?;
        let new_length = sqlx::query_scalar!(
            "INSERT INTO dicks(uid, chat_id, length, updated_at) VALUES ($1, $2, $3, current_timestamp)
                ON CONFLICT (uid, chat_id) DO UPDATE SET length = (dicks.length + $3), updated_at = current_timestamp
                RETURNING length",
                uid, internal_chat_id, increment)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await? as u64;
        Ok(GrowthResult { new_length, pos_in_top })
    }
,
    pub async fn get_top(&self, chat_id: &ChatIdKind, offset: u32, limit: u32) -> anyhow::Result<Vec<Dick>, sqlx::Error> {
        sqlx::query_as!(Dick,
            r#"SELECT length, name as owner_name, updated_at as grown_at,
                    ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                FROM dicks d
                JOIN users using (uid)
                JOIN chats c ON c.id = d.chat_id
                WHERE c.chat_id = $1::bigint OR c.chat_instance = $1::text
                OFFSET $2 LIMIT $3"#,
                chat_id.value() as String, offset as i32, limit as i32)
            .fetch_all(&self.pool)
            .await
    }
,
    pub async fn set_dod_winner(&self, chat_id: &ChatIdPartiality, user_id: UserId, bonus: u32) -> anyhow::Result<Option<GrowthResult>> {
        let uid = user_id.0 as i64;
        let mut tx = self.pool.begin().await?;
        let internal_chat_id = Self::upsert_chat(&mut tx, chat_id).await?;
        let new_length = match Self::grow_dods_dick(&mut tx, internal_chat_id, uid, bonus as i32).await? {
            Some(length) => length,
            None => return Ok(None)
        };
        Self::insert_to_dod_table(&mut tx, internal_chat_id, uid).await?;
        tx.commit().await?;
        let pos_in_top = self.get_position_in_top(internal_chat_id, uid).await? as u64;
        Ok(Some(GrowthResult { new_length, pos_in_top }))
    }
,
    pub async fn get_chat(&self, chat_id: ChatIdKind) -> anyhow::Result<Option<Chat>> {
        sqlx::query_as!(Chat, "SELECT id as internal_id, chat_id, chat_instance FROM Chats
                WHERE chat_id = $1::bigint OR chat_instance = $1::text",
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    async fn upsert_chat(tx: &mut Transaction<'_, Postgres>, chat_id: &ChatIdPartiality) -> anyhow::Result<i64> {
        let (id, instance) = match chat_id {
            ChatIdPartiality::Both(full) => (Some(full.id.0), Some(full.instance.to_owned())),
            ChatIdPartiality::Specific(ChatIdKind::ID(id)) => (Some(id.0), None),
            ChatIdPartiality::Specific(ChatIdKind::Instance(instance)) => (None, Some(instance.to_owned())),
        };
        let chats = sqlx::query_as!(Chat, "SELECT id as internal_id, chat_id, chat_instance FROM Chats
                WHERE chat_id = $1 OR chat_instance = $2",
                id, instance)
            .fetch_all(&mut **tx)
            .await?;
        match chats.len() {
            1 if chats[0].chat_id == id && chats[0].chat_instance == instance => Ok(chats[0].internal_id),
            1 => Self::update_chat(&mut *tx, chats[0].internal_id, id, instance).await,
            0 => Self::create_chat(&mut *tx, id, instance).await,
            2 => Self::merge_chats(&mut *tx, [&chats[0], &chats[1]]).await,
            x => Err(anyhow!("unexpected count of chats ({x}): {chats:?}")),
        }
    }
,
    async fn create_chat(tx: &mut Transaction<'_, Postgres>, chat_id: Option<i64>, chat_instance: Option<String>) -> anyhow::Result<i64> {
        sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, $2) RETURNING id",
                chat_id, chat_instance)
            .fetch_one(&mut **tx)
            .await
            .map_err(|e| e.into())
    }
,
    async fn update_chat(tx: &mut Transaction<'_, Postgres>, internal_id: i64, chat_id: Option<i64>, chat_instance: Option<String>) -> anyhow::Result<i64> {
        sqlx::query!("UPDATE Chats SET chat_id = $2, chat_instance = $3 WHERE id = $1",
                internal_id, chat_id, chat_instance)
            .execute(&mut **tx)
            .await
            .and_then(|res| if res.rows_affected() == 1 {
                    Ok(internal_id)
                } else {
                    Err(sqlx::Error::RowNotFound)
                })
            .map_err(|e| e.into())
    }
,
    async fn merge_chats(tx: &mut Transaction<'_, Postgres>, chats: [&Chat; 2]) -> anyhow::Result<i64> {
        log::info!("merging chats: {chats:?}");
        let state = merge_chat_objects(&chats)?;

        let updated_dicks = sqlx::query!(
            "WITH sum_dicks AS (SELECT uid, sum(length) as length FROM Dicks WHERE chat_id IN ($1, $2) GROUP BY uid)
                    UPDATE Dicks d SET length = sum_dicks.length, bonus_attempts = (bonus_attempts + 1)
                    FROM sum_dicks WHERE chat_id = $1 AND d.uid = sum_dicks.uid",
                state.main.internal_id, state.deleted.0)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        let deleted_dicks = sqlx::query!("DELETE FROM Dicks WHERE chat_id = $1", state.deleted.0)
            .execute(&mut **tx)
            .await?
            .rows_affected();
        if updated_dicks != deleted_dicks {
            return Err(anyhow!("counts of updated {updated_dicks} and deleted {deleted_dicks} dicks are not equal"))
        }

        sqlx::query!("DELETE FROM Chats WHERE id = $1 AND chat_instance = $2",
                state.deleted.0, state.deleted.1)
            .execute(&mut **tx)
            .await
            .and_then(ensure_only_one_row_updated)?;
        sqlx::query!("UPDATE Chats SET chat_instance = $3 WHERE id = $1 AND chat_id = $2",
                state.main.internal_id, state.main.chat_id, state.main.chat_instance)
            .execute(&mut **tx)
            .await
            .and_then(ensure_only_one_row_updated)?;
        Ok(state.main.internal_id)
    }
,
    async fn get_position_in_top(&self, chat_id_internal: i64, uid: i64) -> anyhow::Result<i64> {
        sqlx::query_scalar!(
                r#"SELECT position AS "position!" FROM (
                    SELECT uid, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                    FROM dicks
                    JOIN users using (uid)
                    WHERE chat_id = $1
                ) AS _
                WHERE uid = $2"#,
                chat_id_internal, uid)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.into())
    }
,
    async fn grow_dods_dick(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: i64, bonus: i32) -> anyhow::Result<Option<i32>> {
        sqlx::query_scalar!(
            "UPDATE Dicks SET bonus_attempts = (bonus_attempts + 1), length = (length + $3)
                WHERE chat_id = $1 AND uid = $2
                RETURNING length",
                chat_id_internal, user_id, bonus)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| e.into())
    }
,
    async fn insert_to_dod_table(tx: &mut Transaction<'_, Postgres>, chat_id_internal: i64, user_id: i64) -> anyhow::Result<()> {
        sqlx::query!("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)",
                chat_id_internal, user_id)
            .execute(&mut **tx)
            .await?;
        Ok(())
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
                chat_instance: &chat_instances[0].1,
            },
            deleted: (chat_instances[0].0, &chat_instances[0].1)
        })
    }
}

fn ensure_only_one_row_updated(res: PgQueryResult) -> Result<PgQueryResult, sqlx::Error> {
    if res.rows_affected() == 1 {
        Ok(res)
    } else {
        Err(sqlx::Error::RowNotFound)
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
