use autometrics::autometrics;
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
    /// The bot can't post to this chat: it was kicked, blocked, muted, or the chat is gone. Set by
    /// a failed broadcast, cleared by the next command processed in the chat.
    pub is_unreachable: bool,
}

#[derive(Debug, derive_more::Error, derive_more::Display)]
pub struct NoChatIdError(#[error(not(source))] i64);

/// What became of a chat when Telegram turned its group into a supergroup.
///
/// The `Display` strings are the values of the `outcome` label of the migration metric, so they
/// are a part of the observability contract — renaming a variant renames a time series.
#[derive(Debug, PartialEq, Eq, Clone, Copy, strum_macros::Display, strum_macros::EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum ChatMigrationOutcome {
    /// The row was repointed at the new id, keeping its internal one, so everything the chat had
    /// came along.
    Migrated,
    /// The row was repointed, but it had no `chat_instance` of its own — the chat was never
    /// anchored. Whatever it played through inline mode sits in a separate row keyed by an
    /// instance that the migration has just reissued, so that half is gone while the rest of the
    /// chat came across intact.
    MigratedUnanchored,
    /// Nothing was known by the old `chat_id`. Either the bot had never seen this chat, or it was
    /// only ever known by its `chat_instance` — and that changes with the migration too, which
    /// leaves such a row unreachable for good. The two are indistinguishable from here.
    Untraceable,
    /// Both ids already have a row of their own. Nothing is lost, but the chat's state is split in
    /// two and only a human can decide how to fold it together.
    Conflict,
}

impl ChatMigrationOutcome {
    /// Which of the two "the row moved" outcomes applies, given the instance that row held.
    fn of_migrated(instance: Option<&str>) -> Self {
        instance.map(|_| Self::Migrated).unwrap_or(Self::MigratedUnanchored)
    }
}

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
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn get_chat(&self, chat_id: ChatIdKind) -> anyhow::Result<Option<Chat>> {
        sqlx::query_as!(Chat, "SELECT id as internal_id, chat_id, chat_instance, is_unreachable FROM Chats
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
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn is_anchored(&self, chat_id: &TelegramChatId) -> anyhow::Result<bool> {
        sqlx::query_scalar!(r#"SELECT EXISTS(SELECT 1 FROM Chats
                WHERE chat_id = $1 AND chat_instance IS NOT NULL) AS "exists!""#,
                chat_id.value())
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't check whether the chat with id = {chat_id} is anchored"))
    }
,
    /// Remembers that the bot couldn't post to this chat, so the daily shrink stops trying to
    /// broadcast there. Cleared by [`Self::upsert_chat`] on the next command in the chat.
    ///
    /// Keyed by the Telegram id: that's what the broadcast holds, and a chat known only by its
    /// `chat_instance` is never a broadcast target anyway. Updating no row is fine — the chat may
    /// have been merged or migrated away between the shrink and the send.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn mark_unreachable(&self, chat_id: &TelegramChatId) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Chats SET is_unreachable = true WHERE chat_id = $1", chat_id.value())
            .execute(&self.pool)
            .await
            .map(|_| ())
            .context(format!("couldn't mark the chat with id = {chat_id} as unreachable"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn get_internal_id(&self, chat_id: &ChatIdKind) -> Result<InternalChatId, SearchError<ChatIdKind>> {
        self.get_chat(chat_id.clone()).await
            .map_err(SearchError::Internal)?
            .map(|chat| chat.internal_id)
            .map(InternalChatId::new)
            .ok_or(SearchError::NotFound(chat_id.clone()))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
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
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, lang = ?lang))]
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
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn upsert_chat(&self, chat_id: &ChatIdPartiality) -> anyhow::Result<InternalChatId> {
        let (id, instance) = match chat_id {
            ChatIdPartiality::Both(full, _) if self.features.chats_merging => (Some(full.id.value()), Some(full.instance.to_string())),
            ChatIdPartiality::Both(full, ChatIdSource::Database) => (Some(full.id.value()), None),
            ChatIdPartiality::Both(full, ChatIdSource::InlineQuery) => (None, Some(full.instance.to_string())),
            ChatIdPartiality::Specific(ChatIdKind::ID(id)) => (Some(id.value()), None),
            ChatIdPartiality::Specific(ChatIdKind::Instance(instance)) => (None, Some(instance.to_string())),
        };
        let handled_in_chat = matches!(chat_id,
            ChatIdPartiality::Both(_, ChatIdSource::Database) | ChatIdPartiality::Specific(ChatIdKind::ID(_)));
        let mut tx = self.pool.begin().await?;
        let chats = sqlx::query_as!(Chat, "SELECT id as internal_id, chat_id, chat_instance, is_unreachable FROM Chats
                WHERE chat_id = $1 OR chat_instance = $2",
                id, instance)
            .fetch_all(&mut *tx)
            .await
            .context(format!("couldn't find the chat with id = {chat_id}"))?;
        let was_unreachable = chats.iter().any(|chat| chat.is_unreachable);
        let internal_id = match chats.len() {
            1 if chats[0].chat_id == id && chats[0].chat_instance == instance => Ok(chats[0].internal_id),
            1 => Self::update_chat(&mut tx, chats[0].internal_id, id, instance.as_deref()).await,
            0 => Self::create_chat(&mut tx, id, instance.as_deref()).await,
            2 => Self::merge_chats(&mut tx, [&chats[0], &chats[1]]).await,
            x => bail!("unexpected count of chats ({x}): {chats:?}"),
        }?;
        if handled_in_chat && was_unreachable {
            Self::mark_reachable(&mut tx, internal_id).await?;
        }
        tx.commit().await?;
        Ok(InternalChatId::new(internal_id))
    }
,
    /// Undoes [`Self::mark_unreachable`]. Runs inside the caller's transaction, so it can't clear
    /// the flag for a chat whose upsert then rolls back.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = internal_id))]
    async fn mark_reachable(tx: &mut Transaction<'_, Postgres>, internal_id: i64) -> anyhow::Result<()> {
        log::info!("the chat with id = {internal_id} is reachable again");
        sqlx::query!("UPDATE Chats SET is_unreachable = false WHERE id = $1", internal_id)
            .execute(&mut **tx)
            .await
            .map(|_| ())
            .context(format!("couldn't mark the chat with id = {internal_id} as reachable"))
    }
,
    /// Repoints a chat's row from its old (basic group) id to the new supergroup one after
    /// Telegram migrated the group.
    ///
    /// The row keeps its internal id, so everything referencing the chat — dicks, loans, battle
    /// stats, announcements — follows along untouched; only the external id changes.
    ///
    /// Telegram announces a migration in both chats at once, and the dispatcher hands updates of
    /// different chats to different workers, so the two calls genuinely race. `FOR UPDATE` is what
    /// serializes them: the loser blocks on the locked row, then re-evaluates its `WHERE` against
    /// the committed version and finds the chat no longer known by its old id. That looks exactly
    /// like a chat we never knew, which is why the outcome is decided by looking under the new id
    /// as well — the loser of the race has to report success, not loss.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(old = %old, new = %new))]
    pub async fn migrate_chat_id(
        &self,
        old: &TelegramChatId,
        new: &TelegramChatId,
    ) -> anyhow::Result<ChatMigrationOutcome> {
        let mut tx = self.pool.begin().await?;
        let old_chat = sqlx::query!("SELECT id, chat_instance FROM Chats WHERE chat_id = $1 FOR UPDATE", old.value())
            .fetch_optional(&mut *tx)
            .await
            .context(format!("couldn't look up the chat to migrate from id = {old}"))?;
        let (old_internal_id, old_instance) = match old_chat {
            Some(chat) => (chat.id, chat.chat_instance),
            // Nothing under the old id can mean two very different things, and only one of them is
            // a loss: the row may be gone from there simply because the other announcement of this
            // same migration already repointed it. Its presence under the new id tells them apart.
            None => {
                // The repointed row still holds whatever instance it had before, so the loser of
                // the race reads the same answer the winner reported.
                let migrated = sqlx::query_scalar!("SELECT chat_instance FROM Chats WHERE chat_id = $1", new.value())
                    .fetch_optional(&mut *tx)
                    .await
                    .context(format!("couldn't check whether the chat had already been migrated to {new}"))?;
                return Ok(match migrated {
                    Some(instance) => {
                        log::debug!("the chat migrated from {old} to {new} had already been repointed");
                        ChatMigrationOutcome::of_migrated(instance.as_deref())
                    }
                    None => ChatMigrationOutcome::Untraceable
                })
            }
        };
        let new_internal_id = sqlx::query_scalar!("SELECT id FROM Chats WHERE chat_id = $1", new.value())
            .fetch_optional(&mut *tx)
            .await
            .context(format!("couldn't look up the chat to migrate to id = {new}"))?;

        let outcome = match new_internal_id {
            // Telegram assigns a brand-new id to the supergroup, so this is the only case that
            // normally happens, and exactly one of the two announcements gets here — whichever
            // takes the lock first. The other one returns early above.
            None => {
                sqlx::query!("UPDATE Chats SET chat_id = $2 WHERE id = $1", old_internal_id, new.value())
                    .execute(&mut *tx)
                    .await
                    .map_err(Into::into)
                    .and_then(ensure_only_one_row_updated)
                    .context(format!("couldn't migrate the chat with id = {old_internal_id} from {old} to {new}"))?;
                Self::journal_migration(&mut tx, old_internal_id, old, old_instance.as_deref(), new).await?;
                log::info!("migrated the chat with id = {old_internal_id} from {old} to {new}");
                ChatMigrationOutcome::of_migrated(old_instance.as_deref())
            }
            Some(id) if id == old_internal_id => {
                log::debug!("the chat with id = {old_internal_id} has already been migrated to {new}");
                ChatMigrationOutcome::of_migrated(old_instance.as_deref())
            }
            // Two rows for what is one and the same chat. Folding them together would mean moving
            // dicks, loans and stats across rows, some of which sit behind triggers that forbid
            // updates outright — too destructive to attempt blindly for a case Telegram shouldn't
            // produce. Left for manual resolution instead.
            Some(id) => {
                log::error!("couldn't migrate the chat from {old} to {new}: \
                    the target id is already taken by the chat with id = {id} (the old one has id = {old_internal_id}); \
                    the two rows have to be merged manually");
                ChatMigrationOutcome::Conflict
            }
        };
        tx.commit().await?;
        Ok(outcome)
    }
,
    /// Puts the migration on record — the supergroup's own `chat_instance` isn't part of it, since
    /// a service message never carries one and it would have to be collected from some later
    /// callback, at the cost of a write on every upsert forever.
    ///
    /// `old_chat_instance` is whatever the row held, which is `NULL` for a chat that was never
    /// anchored. A chat migrates once, so an existing record means the journal is being written
    /// twice for one event and the first one stands.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = internal_id, old = %old, new = %new))]
    async fn journal_migration(
        tx: &mut Transaction<'_, Postgres>,
        internal_id: i64,
        old: &TelegramChatId,
        old_instance: Option<&str>,
        new: &TelegramChatId,
    ) -> anyhow::Result<()> {
        sqlx::query!(
            "INSERT INTO Chat_Migrations (internal_id, old_chat_id, old_chat_instance, new_chat_id)
                    VALUES ($1, $2, $3, $4)
                    ON CONFLICT (internal_id) DO NOTHING",
                internal_id, old.value(), old_instance, new.value())
            .execute(&mut **tx)
            .await
            .context(format!("couldn't journal the migration of the chat with id = {internal_id} from {old} to {new}"))?;
        Ok(())
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = ?chat_id, chat_instance = ?chat_instance))]
    async fn create_chat(
        tx: &mut Transaction<'_, Postgres>,
        chat_id: Option<i64>,
        chat_instance: Option<&str>,
    ) -> anyhow::Result<i64> {
        log::info!("creating a chat with chat_id = {chat_id:?} and chat_instance = {chat_instance:?}");
        sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, $2) RETURNING id",
                chat_id, chat_instance)
            .fetch_one(&mut **tx)
            .await
            .context(format!("couldn't create a chat with chat_id = {chat_id:?} or chat_instance = {chat_instance:?}"))
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = internal_id, chat_id = ?chat_id, chat_instance = ?chat_instance))]
    async fn update_chat(
        tx: &mut Transaction<'_, Postgres>,
        internal_id: i64,
        chat_id: Option<i64>,
        chat_instance: Option<&str>,
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
    /// `Loans`, `Announcements`, `Dick_of_Day` and `Stale_Dick_Shrinks` reference `Chats(id)`
    /// without `ON DELETE`, so leaving their rows behind doesn't just orphan them — it makes the
    /// whole merge fail on a foreign key violation. `Battle_Stats` cascades instead, which
    /// silently throws the statistics away.
    /// Every step reports how many rows it carried over, purely for the log line. They all write
    /// through the same transaction, so they run one after another rather than concurrently.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_dependent_rows(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<()> {
        let loans = Self::move_loans(tx, main_id, deleted_id).await?;
        let battle_stats = Self::move_battle_stats(tx, main_id, deleted_id).await?;
        let announcements = Self::move_announcements(tx, main_id, deleted_id).await?;
        let imports = Self::move_imports(tx, main_id, deleted_id).await?;
        let dod = Self::move_dicks_of_the_day(tx, main_id, deleted_id).await?;
        let shrinks = Self::move_shrinks(tx, main_id, deleted_id).await?;
        let migrations = Self::move_chat_migrations(tx, main_id, deleted_id).await?;

        log::info!("moved rows from the chat with id = {deleted_id} to {main_id}: \
            loans: {loans}, battle stats: {battle_stats}, announcements: {announcements}, \
            imports: {imports}, dicks of the day: {dod}, shrinks: {shrinks}, \
            migrations: {migrations}");
        Ok(())
    }
,
    /// Nothing constrains `(uid, chat_id)` here, so the loans can just be repointed.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_loans(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        sqlx::query!("UPDATE Loans SET chat_id = $1 WHERE chat_id = $2",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .map(|res| res.rows_affected())
            .context(format!("couldn't move loans from the chat with id = {deleted_id} to {main_id}"))
    }
,
    /// `(uid, chat_id)` is a primary key, so a user present in both chats needs their counters
    /// folded together instead of one row simply landing on top of the other.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_battle_stats(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        let moved = sqlx::query!(
            "INSERT INTO Battle_Stats (uid, chat_id, battles_total, battles_won, win_streak_current, win_streak_max, acquired_length, lost_length)
                    SELECT uid, $1, battles_total, battles_won, win_streak_current, win_streak_max, acquired_length, lost_length
                        FROM Battle_Stats WHERE chat_id = $2
                    ON CONFLICT (uid, chat_id) DO UPDATE SET
                        battles_total = Battle_Stats.battles_total + EXCLUDED.battles_total,
                        battles_won = Battle_Stats.battles_won + EXCLUDED.battles_won,
                        win_streak_max = GREATEST(Battle_Stats.win_streak_max, EXCLUDED.win_streak_max),
                        acquired_length = Battle_Stats.acquired_length + EXCLUDED.acquired_length,
                        lost_length = Battle_Stats.lost_length + EXCLUDED.lost_length",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move battle stats from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Battle_Stats WHERE chat_id = $1", deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete battle stats of the chat with id = {deleted_id}"))?;
        Ok(moved)
    }
,
    /// Keyed by `(chat_id, language)`; the shown counters add up when both chats saw the same
    /// announcement.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_announcements(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        let moved = sqlx::query!(
            "INSERT INTO Announcements (chat_id, language, hash, times_shown)
                    SELECT $1, language, hash, times_shown FROM Announcements WHERE chat_id = $2
                    ON CONFLICT (chat_id, language) DO UPDATE SET
                        times_shown = Announcements.times_shown + EXCLUDED.times_shown",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move announcements from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Announcements WHERE chat_id = $1", deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete announcements of the chat with id = {deleted_id}"))?;
        Ok(moved)
    }
,
    /// Keyed by `(chat_id, uid)`; an import already done in the main chat wins, so that a merge
    /// can't be used to import a second time.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_imports(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        let moved = sqlx::query!(
            "INSERT INTO Imports (chat_id, uid, original_length)
                    SELECT $1, uid, original_length FROM Imports WHERE chat_id = $2
                    ON CONFLICT (chat_id, uid) DO NOTHING",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move imports from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Imports WHERE chat_id = $1", deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete imports of the chat with id = {deleted_id}"))?;
        Ok(moved)
    }
,
    /// An append-only history keyed by `(chat_id, created_at)`, where a day both chats crowned a
    /// winner keeps the main one's.
    ///
    /// The insertion trigger stamps `created_at` with the current date, which would restamp every
    /// past win with today's, so it has to be muted for the move. The ACCESS EXCLUSIVE lock that
    /// takes lasts until the transaction ends — acceptable for something that happens once in a
    /// chat's lifetime. An early return rolls the muting back along with everything else.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_dicks_of_the_day(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        sqlx::query!("ALTER TABLE Dick_of_Day DISABLE TRIGGER trg_check_dod_timestamp")
            .execute(&mut **tx)
            .await
            .context("couldn't mute the insertion trigger of Dick_of_Day")?;
        let moved = sqlx::query!(
            "INSERT INTO Dick_of_Day (chat_id, winner_uid, created_at)
                    SELECT $1, winner_uid, created_at FROM Dick_of_Day WHERE chat_id = $2
                    ON CONFLICT (chat_id, created_at) DO NOTHING",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move the dicks of the day from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("ALTER TABLE Dick_of_Day ENABLE TRIGGER trg_check_dod_timestamp")
            .execute(&mut **tx)
            .await
            .context("couldn't restore the insertion trigger of Dick_of_Day")?;
        sqlx::query!("DELETE FROM Dick_of_Day WHERE chat_id = $1", deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete the dicks of the day of the chat with id = {deleted_id}"))?;
        Ok(moved)
    }
,
    /// Keyed by `internal_id`, so there is at most one record per row and the surviving chat's own
    /// wins. In practice the row being merged away is the one known only by its instance, and a
    /// migration is only ever journaled for a row known by its `chat_id` — but the journal points
    /// at `Chats(id)` without an `ON DELETE`, so it has to be carried over rather than assumed
    /// absent.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_chat_migrations(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        let moved = sqlx::query!(
            "INSERT INTO Chat_Migrations (internal_id, old_chat_id, old_chat_instance, new_chat_id, migrated_at)
                    SELECT $1, old_chat_id, old_chat_instance, new_chat_id, migrated_at
                        FROM Chat_Migrations WHERE internal_id = $2
                    ON CONFLICT (internal_id) DO NOTHING",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move the migration journal from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("DELETE FROM Chat_Migrations WHERE internal_id = $1", deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete the migration journal of the chat with id = {deleted_id}"))?;
        Ok(moved)
    }
,
    /// An append-only history keyed by `(chat_id, uid, created_at)`. The same user really did
    /// shrink twice that day when both chats recorded it, so the lengths add up.
    ///
    /// The table forbids updates outright, and `ON CONFLICT DO UPDATE` fires `BEFORE UPDATE`
    /// triggers, so summing is only possible with the trigger muted — see
    /// [`Self::move_dicks_of_the_day`] for what that costs.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(main_id = %main_id, deleted_id = %deleted_id))]
    async fn move_shrinks(
        tx: &mut Transaction<'_, Postgres>,
        main_id: InternalChatId,
        deleted_id: InternalChatId,
    ) -> anyhow::Result<u64> {
        sqlx::query!("ALTER TABLE Stale_Dick_Shrinks DISABLE TRIGGER trg_forbid_stale_dick_shrinks_updates")
            .execute(&mut **tx)
            .await
            .context("couldn't mute the update trigger of Stale_Dick_Shrinks")?;
        let moved = sqlx::query!(
            "INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length, created_at)
                    SELECT $1, uid, lost_length, created_at FROM Stale_Dick_Shrinks WHERE chat_id = $2
                    ON CONFLICT (chat_id, uid, created_at) DO UPDATE SET
                        lost_length = Stale_Dick_Shrinks.lost_length + EXCLUDED.lost_length",
                main_id as InternalChatId, deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't move shrinks from the chat with id = {deleted_id} to {main_id}"))?
            .rows_affected();
        sqlx::query!("ALTER TABLE Stale_Dick_Shrinks ENABLE TRIGGER trg_forbid_stale_dick_shrinks_updates")
            .execute(&mut **tx)
            .await
            .context("couldn't restore the update trigger of Stale_Dick_Shrinks")?;
        sqlx::query!("DELETE FROM Stale_Dick_Shrinks WHERE chat_id = $1", deleted_id as InternalChatId)
            .execute(&mut **tx)
            .await
            .context(format!("couldn't delete shrinks of the chat with id = {deleted_id}"))?;
        Ok(moved)
    }
,
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_a = chats[0].internal_id, chat_b = chats[1].internal_id))]
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
        Self::move_dependent_rows(tx,
            InternalChatId::new(state.main.internal_id),
            InternalChatId::new(state.deleted.0)
        ).await?;

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
struct MergeChatsError(Box<[Chat; 2]>, String);

impl std::fmt::Display for MergeChatsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("MergeChatsError ({}): {:?}", self.1, self.0))
    }
}

impl MergeChatsError {
    fn new(chats: &[&Chat; 2], msg: &str) -> Self {
        Self(Box::new(chats.map(Chat::to_owned)), msg.to_owned())
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
            is_unreachable: false,
        };
        let chat2 = Chat {
            internal_id: 2,
            chat_id: None,
            chat_instance: Some(inst.clone()),
            is_unreachable: false,
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
            is_unreachable: false,
        };
        let chat2 = Chat {
            internal_id: 2,
            chat_id: Some(id),
            chat_instance: Some(inst.clone()),
            is_unreachable: false,
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
            is_unreachable: false,
        };
        let chat2 = Chat {
            internal_id: 1,
            chat_id: None,
            chat_instance: Some("one".to_owned()),
            is_unreachable: false,
        };
        let chats = [&chat1, &chat2];
        let res = merge_chat_objects(&chats);

        assert!(res.is_err())
    }
}
