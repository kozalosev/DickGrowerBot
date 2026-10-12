use autometrics::autometrics;
use anyhow::{anyhow, Context};
use futures::TryFutureExt;
use domain_types::traits::SaturatingInto;
use sqlx::{Acquire, Executor, PgConnection, Pool, Postgres, Transaction};
use crate::config::FeatureToggles;
use crate::domain::objects::{Dick, DickOfDayResult, GrowthResult};
use crate::domain::primitives::{Bet, DaysCount, LengthChange, Limit, Offset, UserId, Position, Length, Username};
use crate::domain::primitives::chat::{ChatIdPartiality, ChatIdKind, InternalChatId};
use super::Chats;

/// The database projection of a [`Dick`]. `position` is a `ROW_NUMBER()` (a plain `int8`),
/// so it's decoded as `i64` here and converted to the `Position` domain type at this boundary
/// (`Position` wraps a `u64` and isn't a database type).
#[derive(sqlx::FromRow)]
struct DickEntity {
    length: Length,
    owner_uid: UserId,
    owner_name: String,
    grown_at: chrono::DateTime<chrono::Utc>,
    position: Option<i64>,
}

impl From<DickEntity> for Dick {
    fn from(entity: DickEntity) -> Self {
        Self {
            length: entity.length,
            owner_uid: entity.owner_uid,
            owner_name: entity.owner_name,
            grown_at: entity.grown_at,
            position: entity.position.map(|pos| Position::new(pos.saturating_into())),
        }
    }
}

#[derive(Clone)]
pub struct Dicks {
    pool: Pool<Postgres>,
    chats: Chats,
    features: FeatureToggles,
}

impl Dicks {
    pub fn new(pool: Pool<Postgres>, features: FeatureToggles) -> Self {
        Self {
            chats: Chats::new(pool.clone(), features),
            pool,
            features,
        }
    }

    /// Starts a growth: a transaction that holds the dick's row until it is committed or dropped.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), chat_id = %chat_id))]
    pub async fn begin_growth(&self, uid: UserId, chat_id: &ChatIdPartiality) -> anyhow::Result<Growth> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;
        let mut tx = self.pool.begin().await?;
        Self::lock_dick(&mut tx, internal_chat_id, uid).await?;
        Ok(Growth { tx, dicks: self.clone(), uid, chat: internal_chat_id, chat_kind: chat_id.kind() })
    }

    /// Grows a dick without any perks, creating it on the first play. `None` when the day is
    /// already spent: the caller grew today and has no bought attempt to pay for another.
    #[cfg(test)]
    pub async fn create_or_grow(
        &self,
        uid: UserId,
        chat_id: &ChatIdPartiality,
        increment: LengthChange,
    ) -> anyhow::Result<Option<GrowthResult>> {
        self.begin_growth(uid, chat_id)
            .await?
            .commit_growth(increment)
            .await
    }

    /// The length as the transaction sees it, zero for a dick that doesn't exist yet.
    pub async fn fetch_length_in(
        conn: &mut PgConnection,
        uid: UserId,
        chat_id: InternalChatId,
    ) -> anyhow::Result<Length> {
        sqlx::query_scalar!("SELECT length FROM Dicks WHERE chat_id = $1 AND uid = $2",
                chat_id as InternalChatId, uid as UserId)
            .fetch_optional(conn)
            .await
            .map(|maybe_length| maybe_length.map(Length::new).unwrap_or_default())
            .context(format!("couldn't fetch the length of {uid} in {chat_id}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), chat_id = %chat_id))]
    pub async fn fetch_length(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Length> {
        sqlx::query_scalar!("SELECT d.length FROM Dicks d \
                JOIN Chats c ON d.chat_id = c.id \
                WHERE uid = $1 AND \
                    (c.chat_id = $2::bigint OR c.chat_instance = $2::text)",
                uid as UserId, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_length| maybe_length.map(Length::new).unwrap_or_default())
            .context(format!("couldn't fetch length for {chat_id} and {uid}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = uid.value(), chat_id = %chat_id))]
    pub async fn fetch_dick(&self, uid: UserId, chat_id: &ChatIdKind) -> anyhow::Result<Option<Dick>> {
        sqlx::query_as!(DickEntity,
            r#"SELECT length AS "length: Length", uid AS "owner_uid: UserId", name as owner_name, updated_at as grown_at, position FROM (
                 SELECT uid, name, d.length as length, updated_at, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                   FROM Dicks d
                   JOIN users using (uid)
                   JOIN Chats c ON d.chat_id = c.id
                   WHERE c.chat_id = $2::bigint OR c.chat_instance = $2::text
               ) AS _
               WHERE uid = $1"#,
                uid as UserId, chat_id.value() as String)
            .fetch_optional(&self.pool)
            .await
            .map(|maybe_dick| maybe_dick.map(Dick::from))
            .context(format!("couldn't fetch dick for {chat_id} and {uid}"))
    }

    /// The uids of up to `limit` of the chat's players, longest dick first.
    ///
    /// A sample rather than the roll: the one caller tallies the languages of whoever it gets, and
    /// a tally does not become truer for having every last member in it — while a chat of thousands
    /// would otherwise hand the user-service thousands of ids to look up, per chat, per night.
    /// `length DESC` is not a judgement about who counts, only the order the index is already in.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, limit = %limit))]
    pub async fn get_player_uids_sample(&self, chat_id: InternalChatId, limit: Limit) -> anyhow::Result<Vec<UserId>> {
        sqlx::query_scalar!(
            r#"SELECT d.uid AS "uid: UserId" FROM Dicks d
                WHERE d.chat_id = $1 ORDER BY d.length DESC LIMIT $2"#,
                chat_id as InternalChatId, limit as Limit)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't fetch a sample of player uids for {chat_id}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, offset = %offset, limit = %limit))]
    pub async fn get_top(
        &self,
        chat_id: &ChatIdKind,
        offset: Offset,
        limit: Limit,
        inactivity_days: DaysCount,
    ) -> anyhow::Result<Vec<Dick>> {
        let hide_inactive_zero_length = self.features.hide_inactive_zero_length_from_top;
        sqlx::query_as!(DickEntity,
            r#"SELECT length AS "length: Length", uid AS "owner_uid: UserId", name as owner_name, updated_at as grown_at,
                    ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                FROM dicks d
                JOIN users using (uid)
                JOIN chats c ON c.id = d.chat_id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                  AND (NOT $4::bool OR d.length != 0 OR d.updated_at > current_timestamp - make_interval(days => $5::bigint::int))
                OFFSET $2 LIMIT $3"#,
                chat_id.value() as String, offset as Offset, limit as Limit,
                hide_inactive_zero_length, inactivity_days as DaysCount)
            .fetch_all(&self.pool)
            .await
            .map(|dicks| dicks.into_iter().map(Dick::from).collect())
            .context(format!("couldn't get the top of {chat_id} with offset = {offset} and limit = {limit}"))
    }

    /// Starts the election of the Dick of the Day.
    ///
    /// **One winner a day per chat is the primary key of `Dick_of_Day`**, and the election starts
    /// by claiming that key: a chat that already has today's winner conflicts, and only then is
    /// the name of that winner read. Claiming it first also serialises two elections racing in one
    /// chat, since the second waits on the index rather than reading past it.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = user_id.value(), chat_id = %chat_id))]
    pub async fn begin_dod(&self, chat_id: &ChatIdPartiality, user_id: UserId) -> anyhow::Result<DodStart> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;

        let mut tx = self.pool.begin().await?;
        if !Self::claim_the_day(&mut tx, internal_chat_id, user_id).await? {
            tx.rollback().await?;
            return self.get_dod_winner_name(internal_chat_id)
                .map_ok(DodStart::AlreadyChosen)
                .await
        }
        Self::lock_dick(&mut tx, internal_chat_id, user_id).await?;
        Ok(DodStart::Started(Growth {
            tx,
            dicks: self.clone(),
            uid: user_id,
            chat: internal_chat_id,
            chat_kind: chat_id.kind(),
        }))
    }

    /// Crowns the Dick of the Day without any perks, growing the winner by `bonus`.
    #[cfg(test)]
    pub async fn set_dod_winner(
        &self,
        chat_id: &ChatIdPartiality,
        user_id: UserId,
        bonus: LengthChange,
    ) -> anyhow::Result<DickOfDayResult> {
        match self.begin_dod(chat_id, user_id).await? {
            DodStart::AlreadyChosen(name) => Ok(DickOfDayResult::AlreadyChosen(name)),
            DodStart::Started(growth) => growth.commit_dod(bonus).await,
        }
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = user_id.value(), chat_id = %chat_id, length = %length))]
    pub async fn check_dick(&self, chat_id: &ChatIdKind, user_id: UserId, length: Bet) -> anyhow::Result<bool> {
        sqlx::query_scalar!(r#"SELECT length >= $3 AS "enough!" FROM Dicks d
                JOIN Chats c ON d.chat_id = c.id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                    AND uid = $2"#,
                chat_id.value() as String, user_id as UserId, length as Bet)
            .fetch_optional(&self.pool)
            .map_ok(|opt| opt.unwrap_or(false))
            .await
            .context(format!("couldn't check the dick {chat_id}, {user_id} to have at least {length} cm"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(from = from.value(), to = to.value(), chat_id = %chat_id, length = %length))]
    pub async fn move_length(
        &self,
        chat_id: &ChatIdPartiality,
        from: UserId,
        to: UserId,
        length: Bet,
    ) -> anyhow::Result<(GrowthResult, GrowthResult)> {
        let internal_chat_id = self.chats.upsert_chat(chat_id).await?;
        let winner_change = length.as_length_change_for_winner();
        let loser_change = length.as_length_change_for_loser();

        let mut tx = self.pool.begin().await?;
        let length_from = Self::move_length_for_one_user(&mut tx, internal_chat_id, from, loser_change).await?;
        let length_to = Self::move_length_for_one_user(&mut tx, internal_chat_id, to, winner_change).await?;
        tx.commit().await?;

        let pos_from = self.get_position_in_top(internal_chat_id, from).await?;
        let pos_to = self.get_position_in_top(internal_chat_id, to).await?;
        let gr_from = GrowthResult {
            new_length: length_from,
            pos_in_top: pos_from,
        };
        let gr_to = GrowthResult {
            new_length: length_to,
            pos_in_top: pos_to,
        };
        Ok((gr_from, gr_to))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id_internal, uid = user_id.value(), change = %change))]
    async fn move_length_for_one_user(
        tx: &mut Transaction<'_, Postgres>,
        chat_id_internal: InternalChatId,
        user_id: UserId,
        change: LengthChange,
    ) -> anyhow::Result<Length> {
        sqlx::query_scalar!("UPDATE Dicks SET length = (length + $3) WHERE chat_id = $1 AND uid = $2 RETURNING length",
                    chat_id_internal as InternalChatId, user_id as UserId, change.value())
            .fetch_one(&mut **tx)
            .await
            .map(Length::new)
            .context(format!("couldn't update the length by {change} for {chat_id_internal}, {user_id}"))
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id_internal, uid = uid.value()))]
    async fn get_position_in_top(
        &self,
        chat_id_internal: InternalChatId,
        uid: UserId,
    ) -> anyhow::Result<Option<Position>> {
        if !self.features.top_unlimited {
            return Ok(None)
        }
        sqlx::query_scalar!(
                r#"SELECT position AS "position!" FROM (
                    SELECT uid, ROW_NUMBER() OVER (ORDER BY length DESC, updated_at DESC, name) AS position
                    FROM dicks
                    JOIN users using (uid)
                    WHERE chat_id = $1
                ) AS _
                WHERE uid = $2"#,
                chat_id_internal as InternalChatId, uid as UserId)
            .fetch_one(&self.pool)
            .await
            .map(|pos| Some(Position::new(pos.saturating_into())))
            .context(format!("couldn't get the top for {chat_id_internal} and {uid}"))
    }
    
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = user_id.value(), chat_id = %chat_id, change = %change))]
    pub async fn grow_no_attempts_check(
        &self,
        chat_id: &ChatIdKind,
        user_id: UserId,
        change: LengthChange,
    ) -> anyhow::Result<GrowthResult> {
        let chat_internal_id = self.chats.get_internal_id(chat_id).await?;

        let new_length = Self::grow_no_attempts_check_internal(&self.pool, chat_internal_id, user_id, change).await?
            .ok_or(anyhow!("couldn't find a dick of ({chat_id}, {user_id}) for some reason"))?;
        let pos_in_top = self.get_position_in_top(chat_internal_id, user_id).await?;
        
        Ok(GrowthResult { new_length, pos_in_top })
    }

    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id_internal, uid = user_id.value(), bonus = %bonus))]
    pub(super) async fn grow_no_attempts_check_internal<'c, E>(
        executor: E,
        chat_id_internal: InternalChatId,
        user_id: UserId,
        bonus: LengthChange,
    ) -> anyhow::Result<Option<Length>>
    where E: Executor<'c, Database = Postgres>,
    {
        sqlx::query_scalar!(
            "UPDATE Dicks SET length = (length + $3)
                WHERE chat_id = $1 AND uid = $2
                RETURNING length",
                chat_id_internal as InternalChatId, user_id as UserId, bonus.value())
            .fetch_optional(executor)
            .await
            .map(|maybe_length| maybe_length.map(Length::new))
            .context(format!("couldn't grow the dick without attempts check for {chat_id_internal} and {user_id} by {bonus}"))
    }

    /// Locks the dick's row, if there is one. Taken first in every transaction that also locks the
    /// dick's loan, so that two of them can't wait for each other.
    async fn lock_dick(
        conn: &mut PgConnection,
        chat: InternalChatId,
        uid: UserId,
    ) -> anyhow::Result<()> {
        sqlx::query!("SELECT uid FROM Dicks WHERE chat_id = $1 AND uid = $2 FOR UPDATE",
                chat as InternalChatId, uid as UserId)
            .fetch_optional(conn)
            .await
            .map(|_| ())
            .context(format!("couldn't lock the dick of {uid} in {chat}"))
    }

    /// Crowns `user_id`, unless the chat already has a winner today — which is what `false` means.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id_internal, uid = user_id.value()))]
    async fn claim_the_day(
        tx: &mut Transaction<'_, Postgres>,
        chat_id_internal: InternalChatId,
        user_id: UserId,
    ) -> anyhow::Result<bool> {
        sqlx::query!(
            "INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)
                ON CONFLICT (chat_id, created_at) DO NOTHING",
                chat_id_internal as InternalChatId, user_id as UserId)
            .execute(&mut **tx)
            .await
            .map(|result| result.rows_affected() > 0)
            .context(format!("couldn't insert to DOD table for {chat_id_internal} and {user_id}"))
    }

    /// The name of the chat's winner of today, read after an election has lost the day to them.
    ///
    /// A statement of its own rather than a branch of the insert above: a data-modifying CTE and
    /// the query beside it share one snapshot, so a row the winning election committed after ours
    /// began would be invisible to a `SELECT` in the same statement.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(internal_chat_id = %chat_id_internal))]
    async fn get_dod_winner_name(&self, chat_id_internal: InternalChatId) -> anyhow::Result<Username> {
        sqlx::query_scalar!(
            r#"SELECT u.name AS "name: Username" FROM Dick_of_Day dod
                JOIN Users u ON u.uid = dod.winner_uid
                WHERE dod.chat_id = $1 AND dod.created_at = current_date"#,
                chat_id_internal as InternalChatId)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't fetch the dick of the day of {chat_id_internal}"))
    }
}

/// How the election of a Dick of the Day started.
pub enum DodStart {
    Started(Growth),
    AlreadyChosen(Username),
}

/// One growth of one dick, from the first read of the perks to the commit. Everything done through
/// it is a single transaction: dropping it without a `commit_*` rolls the lot back, which is what
/// a refused growth, a failed perk and a forgotten commit all come to.
pub struct Growth {
    tx: Transaction<'static, Postgres>,
    dicks: Dicks,
    uid: UserId,
    chat: InternalChatId,
    chat_kind: ChatIdKind,
}

impl Growth {
    pub fn uid(&self) -> UserId {
        self.uid
    }

    pub fn chat(&self) -> InternalChatId {
        self.chat
    }

    pub fn chat_kind(&self) -> &ChatIdKind {
        &self.chat_kind
    }

    /// Gives up the growth, releasing its locks at once instead of whenever the connection is next
    /// used.
    pub async fn rollback(self) -> anyhow::Result<()> {
        self.tx.rollback()
            .await
            .context("couldn't roll the growth back")
    }

    /// A savepoint: what is written through it is kept by `commit` and undone by `rollback`.
    pub async fn attempt(&mut self) -> anyhow::Result<Transaction<'_, Postgres>> {
        self.tx.begin()
            .await
            .context("couldn't open a savepoint for an attempt")
    }

    /// Grows the dick, creating it on the first play. `None` when the day is already spent.
    ///
    /// **The once-a-day rule lives in this statement and nowhere else.** An attempt is spent only on
    /// a growth that needs one — the day's first is free whatever the caller has bought.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = self.uid.value(), internal_chat_id = %self.chat, increment = %increment))]
    pub async fn commit_growth(mut self, increment: LengthChange) -> anyhow::Result<Option<GrowthResult>> {
        let (uid, chat) = (self.uid, self.chat);
        let new_length = sqlx::query_scalar!(
            "INSERT INTO dicks(uid, chat_id, length, updated_at) VALUES ($1, $2, $3, current_timestamp)
                ON CONFLICT (uid, chat_id) DO UPDATE SET
                    length = (dicks.length + $3),
                    updated_at = current_timestamp,
                    bonus_attempts = CASE WHEN date(dicks.updated_at) = current_date
                                          THEN dicks.bonus_attempts - 1
                                          ELSE dicks.bonus_attempts END
                WHERE date(dicks.updated_at) <> current_date OR dicks.bonus_attempts > 0
                RETURNING length",
                uid as UserId, chat as InternalChatId, increment.value())
            .fetch_optional(&mut *self.tx)
            .await
            .context(format!("couldn't upsert the dick of {uid} in {chat} with increment of {increment}"))?;
        let Some(new_length) = new_length else {
            self.rollback().await?;
            return Ok(None)
        };
        self.tx.commit().await?;

        let pos_in_top = self.dicks.get_position_in_top(chat, uid).await?;
        Ok(Some(GrowthResult { new_length: Length::new(new_length), pos_in_top }))
    }

    /// Grows the winner of the day by `bonus`. A chat member without a dick can't win.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(uid = self.uid.value(), internal_chat_id = %self.chat, bonus = %bonus))]
    pub async fn commit_dod(mut self, bonus: LengthChange) -> anyhow::Result<DickOfDayResult> {
        let (uid, chat) = (self.uid, self.chat);
        let Some(new_length) = Dicks::grow_no_attempts_check_internal(&mut *self.tx, chat, uid, bonus).await? else {
            self.rollback().await?;
            return Ok(DickOfDayResult::NoDick)
        };
        self.tx.commit().await?;

        let pos_in_top = self.dicks.get_position_in_top(chat, uid).await?;
        Ok(DickOfDayResult::Chosen(GrowthResult { new_length, pos_in_top }))
    }
}
