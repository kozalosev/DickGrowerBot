use autometrics::autometrics;
use anyhow::Context;
use chrono::{DateTime, NaiveDate, Utc};
use crate::domain::primitives::{AttemptsCount, Count, Limit, ScheduledBroadcastId};
use crate::domain::primitives::chat::TelegramChatId;
use crate::repository;

/// How far a summary got. `Created` is the only actionable one; the rest are terminal and stay in
/// the table until the cleaning process removes them, so that a queue which isn't doing its job can
/// be read rather than guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "broadcast_state", rename_all = "snake_case")]
pub enum BroadcastState {
    Created,
    /// The chat got its summary.
    Sent,
    /// Telegram says the bot can't post to that chat at all, which marks the chat too.
    Unreachable,
    /// The summary sat in the queue until it stopped being worth sending.
    Expired,
    /// Every attempt failed for a reason that looked transient and never stopped being one.
    Failed,
}

impl BroadcastState {
    /// The states a row never leaves. What is kept in the table until the cleaning process runs,
    /// and what the gauges of [`crate::metrics::SHRINK_BROADCAST_FINISHED`] are split by.
    pub const TERMINAL: [Self; 4] = [Self::Sent, Self::Unreachable, Self::Expired, Self::Failed];
}

/// A summary a chat is owed, as the worker claims it.
#[derive(Clone, Debug)]
pub struct ScheduledBroadcast {
    pub id: ScheduledBroadcastId,
    /// Read at claim time rather than stored, so a group that became a supergroup meanwhile is
    /// addressed by the id it answers to now.
    pub chat_id: TelegramChatId,
    pub shrink_date: NaiveDate,
    /// When the shrink that owes this summary was committed, which is the summary's age.
    pub created_at: DateTime<Utc>,
    /// Attempts that have already failed, which is what the back-off is computed from.
    pub attempts: AttemptsCount,
}

repository!(ScheduledBroadcasts,
    /// Takes up to `limit` summaries whose time has come, leasing them until `lease_until`.
    ///
    /// The lease is what makes the claim exclusive: the row's lock lives only as long as this one
    /// statement, while the request it leads to takes far longer. A worker that dies mid-batch
    /// leaves its summaries to be claimed again once the lease runs out, rather than for ever.
    ///
    /// A row whose chat has since lost its Telegram id is finished as failed rather than skipped:
    /// the lease runs out, and a row nobody can act on would come back with every tick for ever.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(limit = %limit))]
    pub async fn claim_due(&self, limit: Limit, lease_until: DateTime<Utc>) -> anyhow::Result<Vec<ScheduledBroadcast>> {
        let rows = sqlx::query!(
            r#"UPDATE Scheduled_Shrink_Broadcasts b SET fire_after = $2
                WHERE b.id IN (
                    SELECT id FROM Scheduled_Shrink_Broadcasts
                    WHERE fire_after <= current_timestamp AND finished_at IS NULL
                    ORDER BY fire_after
                    LIMIT $1
                    FOR UPDATE SKIP LOCKED
                )
                RETURNING b.id AS "id: ScheduledBroadcastId",
                          (SELECT c.chat_id FROM Chats c WHERE c.id = b.chat_id) AS "chat_id: TelegramChatId",
                          b.shrink_date, b.created_at, b.attempts AS "attempts!: AttemptsCount""#,
            limit as Limit, lease_until
        )
            .fetch_all(&self.pool)
            .await
            .context("couldn't claim the shrink summaries due for broadcasting")?;

        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(chat_id) = row.chat_id else {
                tracing::error!(id = %row.id, "a queued shrink summary points at a chat with no Telegram id, giving up on it");
                self.finish(row.id, BroadcastState::Failed).await
                    .unwrap_or_else(|e| tracing::error!(id = %row.id, error = format!("{e:#}"),
                        "couldn't give up on the unusable shrink summary"));
                continue
            };
            claimed.push(ScheduledBroadcast {
                id: row.id,
                chat_id,
                shrink_date: row.shrink_date,
                created_at: row.created_at,
                attempts: row.attempts,
            });
        }
        Ok(claimed)
    },

    /// How many summaries are still owed — reported as a gauge, so a queue that stops draining is
    /// visible before the chats are. The finished rows are left out: they are history, and counting
    /// them would make the gauge grow on its own until the cleaning process runs.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn count_pending(&self) -> anyhow::Result<Count<ScheduledBroadcast>> {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: Count<ScheduledBroadcast>"
                FROM Scheduled_Shrink_Broadcasts WHERE finished_at IS NULL"#)
            .fetch_one(&self.pool)
            .await
            .context("couldn't count the pending shrink summaries")
    },

    /// Counts one failed attempt and pushes the row back by `retry_after`.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(id = %id))]
    pub async fn postpone(&self, id: ScheduledBroadcastId, retry_after: DateTime<Utc>) -> anyhow::Result<AttemptsCount> {
        let attempts = sqlx::query_scalar!(
            r#"UPDATE Scheduled_Shrink_Broadcasts SET attempts = attempts + 1, fire_after = $2
                    WHERE id = $1 RETURNING attempts AS "attempts!: AttemptsCount""#,
                id as ScheduledBroadcastId, retry_after)
            .fetch_one(&self.pool)
            .await
            .context("couldn't postpone the shrink summary")?;
        Ok(attempts)
    },

    /// Leaves the row behind in a terminal state instead of dropping it, so that what the worker
    /// did — and what it couldn't do — can be read out of the table until the cleaning process
    /// takes it away.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(id = %id, state = %state))]
    pub async fn finish(&self, id: ScheduledBroadcastId, state: BroadcastState) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE Scheduled_Shrink_Broadcasts SET state = $2, finished_at = current_timestamp WHERE id = $1",
                id as ScheduledBroadcastId, state as BroadcastState)
            .execute(&self.pool)
            .await
            .context("couldn't finish the shrink summary")?;
        Ok(())
    },

    /// Removes the rows that were finished before `older_than`, and says how many went.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn delete_finished(&self, older_than: DateTime<Utc>) -> anyhow::Result<u64> {
        let result = sqlx::query!(
            "DELETE FROM Scheduled_Shrink_Broadcasts WHERE finished_at IS NOT NULL AND finished_at < $1",
                older_than)
            .execute(&self.pool)
            .await
            .context("couldn't clean the finished shrink summaries up")?;
        Ok(result.rows_affected())
    }
);
