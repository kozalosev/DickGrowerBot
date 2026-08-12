use autometrics::autometrics;
use anyhow::Context;
use chrono::{DateTime, NaiveDate, Utc};
use crate::domain::primitives::{Count, DaysCount, Length, Limit, Offset, Ratio, UserId, Username};
use crate::domain::primitives::chat::{ChatIdKind, InternalChatId};
use crate::repo::Chat;
use crate::repository;

/// What one batch of the daily shrink did. The shrinks themselves are in `Stale_Dick_Shrinks` and
/// the summaries they owe are in `Scheduled_Shrink_Broadcasts`, so nothing but the counts has to
/// travel back: at a million victims a day, the rows would be the run's whole memory footprint.
///
/// The three delivery counts partition `victims`, and they are what the daily-shrink metrics are
/// split by. A victim is one shrunk dick, not one user: the same person is counted once per chat
/// they play in, which is also how the summaries are addressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShrinkBatchOutcome {
    pub victims: Count<RecentShrink>,
    /// Victims whose chat got a row in the broadcast queue.
    pub to_broadcast: Count<RecentShrink>,
    /// Victims of chats the bot can't message proactively at all; the `shrinks` command is the only
    /// way they get to see it.
    pub inline_only: Count<RecentShrink>,
    /// Victims of chats the bot couldn't post to last time it tried.
    pub unreachable: Count<RecentShrink>,
    /// Chats that got a row, which is how many messages the broadcast will send.
    pub chats_queued: Count<Chat>,
    /// Chats the bot is known to have lost access to, so nothing was queued for them.
    pub chats_skipped: Count<Chat>,
}

impl std::ops::AddAssign for ShrinkBatchOutcome {
    fn add_assign(&mut self, other: Self) {
        self.victims += other.victims;
        self.to_broadcast += other.to_broadcast;
        self.inline_only += other.inline_only;
        self.unreachable += other.unreachable;
        self.chats_queued += other.chats_queued;
        self.chats_skipped += other.chats_skipped;
    }
}

/// The log only stores how much was lost, so `length` (the owner's *current* length) comes from a
/// `Dicks` join — the post-shrink value when read moments after the daily run, self-updating later.
pub struct RecentShrink {
    pub uid: UserId,
    pub owner_name: Username,
    pub lost_length: Length,
    pub length: Length,
}

/// Named rather than a `(NaiveDate, NaiveDate)` pair: the two are trivially swappable, and the
/// list runs newest-first, so "previous"/"next" would be ambiguous about which way they point.
#[derive(Clone, Copy)]
pub struct AdjacentDates {
    pub older: Option<NaiveDate>,
    pub newer: Option<NaiveDate>,
}

repository!(Shrinks,
    /// One page of chats, in id order, for the run to walk. `after` is the last id of the previous
    /// page, so the caller keeps nothing but that number.
    ///
    /// Deliberately not "the chats that have something to shrink": that question needs a `DISTINCT`
    /// over every stale dick in the database — around a million rows aggregated down to a couple of
    /// hundred thousand ids — and it excludes about one chat in eight, because nearly every chat has
    /// a neglected dick in it. Reading the primary key instead is an index scan, and a page whose
    /// chats turn out to have nothing stale simply shrinks nothing.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(after = ?after, limit = %limit))]
    pub async fn select_chats_page(
        &self,
        after: Option<InternalChatId>,
        limit: Limit,
    ) -> anyhow::Result<Vec<InternalChatId>> {
        sqlx::query_scalar!(
            r#"SELECT id AS "id: InternalChatId" FROM Chats
                WHERE id > $1 ORDER BY id LIMIT $2"#,
                after.unwrap_or(InternalChatId::new(0)) as InternalChatId, limit as Limit)
            .fetch_all(&self.pool)
            .await
            .context("couldn't read a page of chats to shrink")
    },

    /// Shrinks the stale dicks of `chat_ids`, logs each shrink into `Stale_Dick_Shrinks` and queues
    /// one broadcast per chat that can be messaged. The whole thing is one statement: Postgres runs
    /// the unreferenced data-modifying CTEs to completion.
    ///
    /// Queueing in the same statement is the durability of the broadcast: there is no moment at
    /// which a shrink is committed and the summary it owes is not. What the queue then does with
    /// the row — when to send it, how often to retry — is nobody's business here.
    ///
    /// Taking the chats as an argument bounds both the locks and the result: the run walks them in
    /// batches, so a `/grow` at midnight waits behind one batch instead of behind every victim in
    /// the database, and a batch that fails costs its own chats rather than the whole day.
    ///
    /// The full `ratio` doesn't apply from day one of staleness: it ramps up linearly over
    /// `ramp_up_days`, starting at `ratio / ramp_up_days` on the first overdue day and reaching
    /// the full `ratio` once a dick has been overdue for `ramp_up_days` days (and staying there
    /// afterwards) — so neglect is punished gradually rather than with one abrupt cut the moment
    /// the grace period lapses. `ramp_up_days <= 1` reproduces the old instant-full-ratio behavior.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chats = chat_ids.len(), ratio = %ratio, grace_days = %grace_days, ramp_up_days = %ramp_up_days))]
    pub async fn perform_daily_shrink(
        &self,
        chat_ids: &[InternalChatId],
        ratio: Ratio,
        grace_days: DaysCount,
        ramp_up_days: DaysCount,
    ) -> anyhow::Result<ShrinkBatchOutcome> {
        let outcome = sqlx::query_as!(ShrinkBatchOutcome,
            r#"WITH victims AS (
                    SELECT d.uid, d.chat_id,
                           LEAST(d.length, GREATEST(1, CEIL(d.length * $1::double precision * LEAST(1.0,
                               (EXTRACT(DAY FROM (current_timestamp - d.updated_at))::int - $2::bigint::int + 1)::double precision
                                   / GREATEST($3::bigint::int, 1)
                           ))::bigint)) AS loss
                    FROM Dicks d
                    WHERE d.chat_id = ANY($4)
                      AND d.length > 0
                      AND d.updated_at <= current_timestamp - make_interval(days => $2::bigint::int)
                ),
                updated AS (
                    UPDATE Dicks d SET length = d.length - v.loss, bonus_attempts = d.bonus_attempts + 1
                    FROM victims v WHERE d.uid = v.uid AND d.chat_id = v.chat_id
                    RETURNING d.uid, d.chat_id, v.loss AS loss
                ),
                logged AS (
                    INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length)
                    SELECT chat_id, uid, loss FROM updated
                ),
                classified AS (
                    SELECT u.uid, u.chat_id, c.chat_id IS NOT NULL AS messageable, c.is_unreachable
                    FROM updated u JOIN Chats c ON c.id = u.chat_id
                ),
                queued AS (
                    INSERT INTO Scheduled_Shrink_Broadcasts (chat_id, shrink_date)
                    SELECT DISTINCT chat_id, current_date FROM classified
                    WHERE messageable AND NOT is_unreachable
                    ON CONFLICT DO NOTHING
                    RETURNING chat_id
                )
                SELECT count(*) AS "victims!: Count<RecentShrink>",
                       count(*) FILTER (WHERE messageable AND NOT is_unreachable) AS "to_broadcast!: Count<RecentShrink>",
                       count(*) FILTER (WHERE NOT messageable) AS "inline_only!: Count<RecentShrink>",
                       count(*) FILTER (WHERE messageable AND is_unreachable) AS "unreachable!: Count<RecentShrink>",
                       (SELECT count(*) FROM queued) AS "chats_queued!: Count<Chat>",
                       count(DISTINCT chat_id) FILTER (WHERE messageable AND is_unreachable) AS "chats_skipped!: Count<Chat>"
                FROM classified"#,
                ratio as Ratio, grace_days as DaysCount, ramp_up_days as DaysCount,
                chat_ids as &[InternalChatId])
            .fetch_one(&self.pool)
            .await
            .context("couldn't perform the daily shrink")?;
        Ok(outcome)
    },

    /// When the last shrink was logged, as the moment of the UTC midnight it belongs to. `None`
    /// before the first run ever.
    ///
    /// Published as a gauge, which is the only shape that answers "is the scheduler still alive?"
    /// across a restart: the table remembers, where a counter in this process does not.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn get_last_shrink_timestamp(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        sqlx::query_scalar!(
            r#"SELECT (max(created_at)::timestamp AT TIME ZONE 'UTC') AS "at?" FROM Stale_Dick_Shrinks"#)
            .fetch_one(&self.pool)
            .await
            .context("couldn't read the time of the last shrink")
    },

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, date = %date, offset = %offset, limit = %limit))]
    pub async fn get_shrinks_for_date(
        &self,
        chat_id: &ChatIdKind,
        date: NaiveDate,
        offset: Offset,
        limit: Limit,
    ) -> anyhow::Result<Vec<RecentShrink>> {
        sqlx::query_as!(RecentShrink,
            r#"SELECT s.uid AS "uid: UserId", usr.name AS "owner_name: Username",
                       s.lost_length AS "lost_length!: Length", d.length AS "length!: Length"
                FROM Stale_Dick_Shrinks s
                JOIN Users usr USING (uid)
                JOIN Dicks d ON d.uid = s.uid AND d.chat_id = s.chat_id
                JOIN Chats c ON c.id = s.chat_id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                  AND s.created_at = $2
                ORDER BY s.lost_length DESC
                OFFSET $3 LIMIT $4"#,
                chat_id.value() as String, date, offset as Offset, limit as Limit)
            .fetch_all(&self.pool)
            .await
            .context(format!("couldn't fetch shrinks of {chat_id} for {date}"))
    },

    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id))]
    pub async fn get_latest_shrink_date(
        &self,
        chat_id: &ChatIdKind,
    ) -> anyhow::Result<Option<NaiveDate>> {
        sqlx::query_scalar!(
            r#"SELECT MAX(s.created_at) AS "created_at"
                FROM Stale_Dick_Shrinks s
                JOIN Chats c ON c.id = s.chat_id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)"#,
                chat_id.value() as String)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't fetch the latest shrink date for {chat_id}"))
    },

    /// Nearest older and newer dates with logged shrinks. Two scalar lookups rather than a date
    /// list, so day-navigation stays constant-work however deep the history goes.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(chat_id = %chat_id, date = %date))]
    pub async fn get_adjacent_shrink_dates(
        &self,
        chat_id: &ChatIdKind,
        date: NaiveDate,
    ) -> anyhow::Result<AdjacentDates> {
        let older = sqlx::query_scalar!(
            r#"SELECT MAX(s.created_at) AS "created_at"
                FROM Stale_Dick_Shrinks s
                JOIN Chats c ON c.id = s.chat_id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                  AND s.created_at < $2"#,
                chat_id.value() as String, date)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't fetch the older shrink date for {chat_id}"))?;
        let newer = sqlx::query_scalar!(
            r#"SELECT MIN(s.created_at) AS "created_at"
                FROM Stale_Dick_Shrinks s
                JOIN Chats c ON c.id = s.chat_id
                WHERE (c.chat_id = $1::bigint OR c.chat_instance = $1::text)
                  AND s.created_at > $2"#,
                chat_id.value() as String, date)
            .fetch_one(&self.pool)
            .await
            .context(format!("couldn't fetch the newer shrink date for {chat_id}"))?;
        Ok(AdjacentDates { older, newer })
    }
);
