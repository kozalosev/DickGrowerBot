use std::fmt::Formatter;
use std::str::FromStr;
use anyhow::{bail, Context};
use sqlx::{Postgres, Transaction};
use crate::domain::primitives::SupportedLanguage;
use crate::domain::primitives::chat::{ChatIdFull, ChatIdKind, ChatIdPartiality, ChatIdSource, InternalChatId, TelegramChatId, TelegramChatInstanceId};
use crate::repo::ensure_only_one_row_updated;
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
            (Some(id), Some(instance)) => Ok(ChatIdPartiality::Both(
                ChatIdFull { id: TelegramChatId::new(id), instance: TelegramChatInstanceId::new(instance) },
                ChatIdSource::Database
            )),
            (Some(id), None) => Ok(ChatIdPartiality::Specific(
                ChatIdKind::ID(TelegramChatId::new(id))
            )),
            (None, Some(instance)) => Ok(ChatIdPartiality::Specific(
                ChatIdKind::Instance(TelegramChatInstanceId::new(instance))
            )),
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
    /// Tells whether a chat is *anchored*: its row holds both the real `chat_id` and the
    /// `chat_instance`, so inline invocations (keyed by `chat_instance`) resolve to the very
    /// same row as command invocations (keyed by `chat_id`).
    ///
    /// Only meaningful for legacy (basic) groups: their `inline_message_id` doesn't encode the
    /// chat, so the pairing can't be recovered later and has to be captured up front by a tap
    /// on an in-chat button (see the `setup` callback).
    pub async fn is_anchored(&self, chat_id: &TelegramChatId) -> anyhow::Result<bool> {
        sqlx::query_scalar!(r#"SELECT EXISTS(SELECT 1 FROM Chats
                WHERE chat_id = $1 AND chat_instance IS NOT NULL) AS "exists!""#,
                chat_id.value())
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't check whether the chat with id = {chat_id} is anchored"))
    }
,
    pub async fn get_internal_id(&self, chat_id: &ChatIdKind) -> Result<InternalChatId, SearchError<ChatIdKind>> {
        self.get_chat(chat_id.clone()).await
            .map_err(SearchError::Internal)?
            .map(|chat| chat.internal_id)
            .map(InternalChatId::new)
            .ok_or(SearchError::NotFound(chat_id.clone()))
    }
,
    pub async fn get_chat_language(&self, chat_id: &ChatIdKind) -> anyhow::Result<Option<SupportedLanguage>> {
        sqlx::query_scalar!(
                "SELECT settings->>'language' FROM Chats
                    WHERE chat_id = $1::bigint OR chat_instance = $1::text",
                chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .context(format!("couldn't get the language of the chat with id = {chat_id}"))?
            .flatten()
            .and_then(|code| SupportedLanguage::from_str(&code)
                .map_err(|_| log::warn!("unknown chat language {code:?} stored for {chat_id}"))
                .ok())
            .map(Ok)
            .transpose()
    }
,
    pub async fn set_chat_language(&self, chat_id: &ChatIdPartiality, lang: Option<SupportedLanguage>) -> anyhow::Result<()> {
        let internal_id = self.upsert_chat(chat_id).await?;
        match lang {
            Some(lang) => sqlx::query!(
                    "UPDATE Chats SET settings = jsonb_set(settings, '{language}', to_jsonb($2::text)) WHERE id = $1",
                    internal_id as InternalChatId, lang as SupportedLanguage)
                .execute(&self.pool)
                .await
                .context(format!("couldn't set the language of the chat {chat_id} to {lang}"))?,
            None => sqlx::query!("UPDATE Chats SET settings = settings - 'language' WHERE id = $1",
                    internal_id as InternalChatId)
                .execute(&self.pool)
                .await
                .context(format!("couldn't clear the language of the chat {chat_id}"))?,
        };
        Ok(())
    }
,
    pub async fn upsert_chat(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<InternalChatId> {
        let (id, instance) = match chat_id {
            ChatIdPartiality::Both(full, _) if self.features.chats_merging => (Some(full.id.value()), Some(full.instance.to_string())),
            ChatIdPartiality::Both(full, ChatIdSource::Database) => (Some(full.id.value()), None),
            ChatIdPartiality::Both(full, ChatIdSource::InlineQuery) => (None, Some(full.instance.to_string())),
            ChatIdPartiality::Specific(ChatIdKind::ID(id)) => (Some(id.value()), None),
            ChatIdPartiality::Specific(ChatIdKind::Instance(instance)) => (None, Some(instance.to_string())),
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
        Ok(InternalChatId::new(internal_id))
    }
,
    /// Repoints a chat's row from its old (basic group) id to the new supergroup one after
    /// Telegram migrated the group.
    ///
    /// The row keeps its internal id, so everything referencing the chat — dicks, loans, battle
    /// stats, announcements — follows along untouched; only the external id changes.
    pub async fn migrate_chat_id(&self, old: &TelegramChatId, new: &TelegramChatId) -> anyhow::Result<()> {
        let mut tx = self.pool.begin().await?;
        let old_internal_id = sqlx::query_scalar!("SELECT id FROM Chats WHERE chat_id = $1", old.value())
            .fetch_optional(&mut *tx)
            .await
            .context(format!("couldn't look up the chat to migrate from id = {old}"))?;
        let old_internal_id = match old_internal_id {
            Some(id) => id,
            // the bot has never been used in the group before the migration — nothing to move
            None => return Ok(())
        };
        let new_internal_id = sqlx::query_scalar!("SELECT id FROM Chats WHERE chat_id = $1", new.value())
            .fetch_optional(&mut *tx)
            .await
            .context(format!("couldn't look up the chat to migrate to id = {new}"))?;

        match new_internal_id {
            // Telegram assigns a brand-new id to the supergroup, so this is the only case that
            // normally happens; both updates of a migration converge on it (the second one finds
            // no row for `old` anymore and returns early above).
            None => {
                sqlx::query!("UPDATE Chats SET chat_id = $2 WHERE id = $1", old_internal_id, new.value())
                    .execute(&mut *tx)
                    .await
                    .map_err(Into::into)
                    .and_then(ensure_only_one_row_updated)
                    .context(format!("couldn't migrate the chat with id = {old_internal_id} from {old} to {new}"))?;
                log::info!("migrated the chat with id = {old_internal_id} from {old} to {new}");
            }
            Some(id) if id == old_internal_id => {
                log::debug!("the chat with id = {old_internal_id} has already been migrated to {new}")
            }
            // Two rows for what is one and the same chat. Folding them together would mean moving
            // dicks, loans and stats across rows, some of which sit behind triggers that forbid
            // updates outright — too destructive to attempt blindly for a case Telegram shouldn't
            // produce. Left for manual resolution instead.
            Some(id) => log::error!("couldn't migrate the chat from {old} to {new}: \
                the target id is already taken by the chat with id = {id} (the old one has id = {old_internal_id}); \
                the two rows have to be merged manually")
        };
        tx.commit().await?;
        Ok(())
    }
,
    async fn create_chat(
        tx: &mut Transaction<'_, Postgres>,
        chat_id: Option<i64>,
        chat_instance: Option<String>,
    ) -> anyhow::Result<i64> {
        log::info!("creating a chat with chat_id = {chat_id:?} and chat_instance = {chat_instance:?}");
        sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, $2) RETURNING id",
                chat_id, chat_instance)
            .fetch_one(&mut **tx)
            .await
            .context(format!("couldn't create a chat with chat_id = {chat_id:?} or chat_instance = {chat_instance:?}"))
    }
,
    async fn update_chat(
        tx: &mut Transaction<'_, Postgres>,
        internal_id: i64,
        chat_id: Option<i64>,
        chat_instance: Option<String>,
    ) -> anyhow::Result<i64> {
        log::debug!("updating the chat with id = {internal_id}, chat_id = {chat_id:?}, and chat_instance = {chat_instance:?}");
        sqlx::query!("UPDATE Chats SET chat_id = coalesce($2, chat_id), chat_instance = coalesce($3, chat_instance) WHERE id = $1",
                internal_id, chat_id, chat_instance)
            .execute(&mut **tx)
            .await
            .map(|_| internal_id)
            .context(format!("couldn't update the chat with id = {internal_id} to chat_id = {chat_id:?}, chat_instance = {chat_instance:?}))"))
    }
,
    /// Carries everything that references the chat being merged away over to the surviving row.
    ///
    /// `Loans` and `Announcements` reference `Chats(id)` without `ON DELETE`, so leaving their
    /// rows behind doesn't just orphan them — it makes the whole merge fail on a foreign key
    /// violation. `Battle_Stats` cascades instead, which silently throws the statistics away.
    ///
    /// `Dick_of_Day` is deliberately left alone: it sits behind a trigger that forbids updates
    /// outright, and re-inserting rewrites `created_at`, which would corrupt the history. It has
    /// no foreign key, so its rows are merely orphaned rather than blocking anything.
    async fn move_dependent_rows(tx: &mut Transaction<'_, Postgres>, main_id: i64, deleted_id: i64) -> anyhow::Result<()> {
        // nothing constrains (uid, chat_id) here, so the loans can just be repointed
        let loans = sqlx::query!("UPDATE Loans SET chat_id = $1 WHERE chat_id = $2", main_id, deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move loans from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();

        // (uid, chat_id) is a primary key, so a user present in both chats needs their counters folded
        let battle_stats = sqlx::query!(
            "INSERT INTO Battle_Stats (uid, chat_id, battles_total, battles_won, win_streak_current, win_streak_max, acquired_length, lost_length)
                    SELECT uid, $1, battles_total, battles_won, win_streak_current, win_streak_max, acquired_length, lost_length
                        FROM Battle_Stats WHERE chat_id = $2
                    ON CONFLICT (uid, chat_id) DO UPDATE SET
                        battles_total = Battle_Stats.battles_total + EXCLUDED.battles_total,
                        battles_won = Battle_Stats.battles_won + EXCLUDED.battles_won,
                        win_streak_max = GREATEST(Battle_Stats.win_streak_max, EXCLUDED.win_streak_max),
                        acquired_length = Battle_Stats.acquired_length + EXCLUDED.acquired_length,
                        lost_length = Battle_Stats.lost_length + EXCLUDED.lost_length",
                main_id, deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move battle stats from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Battle_Stats WHERE chat_id = $1", deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete battle stats of the chat with id = {deleted_id}"))?;

        // keyed by (chat_id, language); the shown counters add up when both chats saw the same one
        let announcements = sqlx::query!(
            "INSERT INTO Announcements (chat_id, language, hash, times_shown)
                    SELECT $1, language, hash, times_shown FROM Announcements WHERE chat_id = $2
                    ON CONFLICT (chat_id, language) DO UPDATE SET
                        times_shown = Announcements.times_shown + EXCLUDED.times_shown",
                main_id, deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move announcements from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Announcements WHERE chat_id = $1", deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete announcements of the chat with id = {deleted_id}"))?;

        // keyed by (chat_id, uid); an import already done in the main chat wins, so that a merge
        // can't be used to import a second time
        let imports = sqlx::query!(
            "INSERT INTO Imports (chat_id, uid, original_length)
                    SELECT $1, uid, original_length FROM Imports WHERE chat_id = $2
                    ON CONFLICT (chat_id, uid) DO NOTHING",
                main_id, deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move imports from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Imports WHERE chat_id = $1", deleted_id)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete imports of the chat with id = {deleted_id}"))?;

        log::info!("moved rows from the chat with id = {deleted_id} to {main_id}: \
            loans: {loans}, battle stats: {battle_stats}, announcements: {announcements}, imports: {imports}");
        Ok(())
    }
,
    async fn merge_chats(tx: &mut Transaction<'_, Postgres>, chats: [&Chat; 2]) -> anyhow::Result<i64> {
        let state = merge_chat_objects(&chats)?;
        // Moving the dicks over rather than summing into the surviving rows: a user who only ever
        // played through inline mode has no row in the surviving chat at all, and updating in
        // place would skip them, leaving their length to be deleted below.
        //
        // `bonus_attempts` is incremented on both paths because the trigger on Dicks decrements it
        // once per write; the increment merely cancels that out. It has to be spelled out again in
        // the conflict branch: the BEFORE INSERT trigger runs before the conflict is detected, so
        // by then `EXCLUDED` already carries the decremented value.
        let updated_dicks = sqlx::query!(
            "INSERT INTO Dicks (uid, chat_id, length, bonus_attempts, updated_at)
                    SELECT uid, $1, length, bonus_attempts + 1, updated_at FROM Dicks WHERE chat_id = $2
                    ON CONFLICT (chat_id, uid) DO UPDATE SET
                        length = Dicks.length + EXCLUDED.length,
                        bonus_attempts = Dicks.bonus_attempts + EXCLUDED.bonus_attempts + 1,
                        updated_at = GREATEST(Dicks.updated_at, EXCLUDED.updated_at)",
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
        Self::move_dependent_rows(tx, state.main.internal_id, state.deleted.0).await?;

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
