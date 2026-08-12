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

    /// How many rows are finished but not yet cleaned, by state — the queue's own history, and the
    /// first thing to look at when the summaries stop arriving.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn count_finished(&self) -> anyhow::Result<Vec<(BroadcastState, Count<ScheduledBroadcast>)>> {
        let rows = sqlx::query!(
            r#"SELECT state AS "state: BroadcastState", count(*) AS "count!: Count<ScheduledBroadcast>"
                FROM Scheduled_Shrink_Broadcasts WHERE finished_at IS NOT NULL GROUP BY state"#)
            .fetch_all(&self.pool)
            .await
            .context("couldn't count the finished shrink summaries")?;
        Ok(rows.into_iter().map(|row| (row.state, row.count)).collect())
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

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use chrono::Utc;
    use sqlx::{Pool, Postgres};
    use super::*;
    use crate::repo::test::fresh_db;

    /// A lease long enough that nothing under test can outlive it.
    fn lease() -> DateTime<Utc> {
        Utc::now() + Duration::from_secs(600)
    }

    /// A chat with a Telegram id, which is what the queue's foreign key points at.
    async fn chat(db: &Pool<Postgres>, telegram_id: i64) -> i64 {
        sqlx::query_scalar!("INSERT INTO Chats (chat_id) VALUES ($1) RETURNING id", telegram_id)
            .fetch_one(db).await.expect("couldn't create the chat")
    }

    /// The enqueue is a CTE of the shrinking statement in production; here it is spelled out, so
    /// that these tests are about the queue rather than about the shrink.
    async fn queue(db: &Pool<Postgres>, internal_chat_id: i64, days_ago: i32) {
        sqlx::query!(
            "INSERT INTO Scheduled_Shrink_Broadcasts (chat_id, shrink_date, created_at) \
                VALUES ($1, current_date - $2::int, current_timestamp - make_interval(days => $2)) \
                ON CONFLICT DO NOTHING",
            internal_chat_id, days_ago)
            .execute(db).await.expect("couldn't queue the summary");
    }

    #[tokio::test]
    async fn a_queued_summary_is_claimed_with_the_chat_it_is_owed_to() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        let internal_id = chat(&db, -1001234567890).await;
        queue(&db, internal_id, 0).await;

        let claimed = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the summaries");

        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].chat_id, TelegramChatId::new(-1001234567890));
        assert_eq!(claimed[0].shrink_date, Utc::now().date_naive());
        assert_eq!(claimed[0].attempts, 0);
    }

    /// The whole point of the lease: a summary being sent is invisible to everyone else, and
    /// nothing here holds a lock long enough to do that on its own.
    #[tokio::test]
    async fn a_claimed_summary_is_not_claimed_again() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        queue(&db, chat(&db, -1001234567890).await, 0).await;

        let claimed = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert_eq!(claimed.len(), 1);
        let claimed_again = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert!(claimed_again.is_empty());
    }

    /// A worker that dies mid-batch must not take its summaries down with it — which is the whole
    /// reason the queue exists.
    #[tokio::test]
    async fn a_summary_of_an_expired_lease_comes_back() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        queue(&db, chat(&db, -1001234567890).await, 0).await;

        let expired = Utc::now() - Duration::from_secs(1);
        let claimed = repo.claim_due(Limit::new(10), expired)
            .await.expect("couldn't claim the summaries");
        assert_eq!(claimed.len(), 1);
        let claimed_again = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert_eq!(claimed_again.len(), 1);
        assert_eq!(claimed_again[0].id, claimed[0].id);
    }

    /// Two runs of the same day owe the chat one message, not two. The unique index is what says so.
    #[tokio::test]
    async fn the_same_chat_and_day_is_queued_once() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        let internal_id = chat(&db, -1001234567890).await;

        queue(&db, internal_id, 0).await;
        queue(&db, internal_id, 0).await;

        let pending = repo.count_pending()
            .await.expect("couldn't count the pending summaries");
        assert_eq!(pending, 1);
    }

    /// Yesterday's summary and today's are different messages, so both are owed.
    #[tokio::test]
    async fn each_day_is_queued_on_its_own() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        let internal_id = chat(&db, -1001234567890).await;

        queue(&db, internal_id, 0).await;
        queue(&db, internal_id, 1).await;

        let pending = repo.count_pending().await.expect("couldn't count the pending summaries");
        assert_eq!(pending, 2);
    }

    #[tokio::test]
    async fn a_postponed_summary_counts_its_attempts() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        queue(&db, chat(&db, -1001234567890).await, 0).await;
        let claimed = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");

        let attempts = repo.postpone(claimed[0].id, Utc::now() - Duration::from_secs(1))
            .await.expect("couldn't postpone the summary");
        assert_eq!(attempts, 1);

        // The count the back-off is computed from has to survive the round trip, or every attempt
        // would rest as long as the first.
        let claimed_again = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert_eq!(claimed_again.len(), 1);
        assert_eq!(claimed_again[0].attempts, 1);
    }

    /// A finished row stays in the table as the account of what happened, but is out of the
    /// worker's way and out of the gauge that says how much work is left.
    #[tokio::test]
    async fn a_finished_summary_is_kept_but_never_claimed() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        queue(&db, chat(&db, -1001234567890).await, 0).await;
        queue(&db, chat(&db, -1009876543210).await, 0).await;
        let claimed = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert_eq!(claimed.len(), 2);

        repo.finish(claimed[0].id, BroadcastState::Sent)
            .await.expect("couldn't finish the summary");
        repo.finish(claimed[1].id, BroadcastState::Unreachable)
            .await.expect("couldn't finish the summary");

        let claimed_again = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert!(claimed_again.is_empty());
        let pending = repo.count_pending()
            .await.expect("couldn't count the pending summaries");
        assert_eq!(pending, 0);

        let mut finished = repo.count_finished()
            .await.expect("couldn't count the finished summaries");
        finished.sort_by_key(|(state, _)| state.to_string());
        assert_eq!(finished, vec![
            (BroadcastState::Sent, Count::<ScheduledBroadcast>::new(1)),
            (BroadcastState::Unreachable, Count::<ScheduledBroadcast>::new(1)),
        ]);
    }

    /// Every terminal state has to survive the trip to the database and back, or a row would end up
    /// counted under the wrong ending.
    #[tokio::test]
    async fn every_terminal_state_survives_the_round_trip() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        for (i, _) in BroadcastState::TERMINAL.iter().enumerate() {
            let telegram_id = -1001234567890 - i64::try_from(i).expect("the index fits");
            queue(&db, chat(&db, telegram_id).await, 0).await;
        }
        let claimed = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");

        for (broadcast, state) in claimed.iter().zip(BroadcastState::TERMINAL) {
            repo.finish(broadcast.id, state)
                .await.expect("couldn't finish the summary");
        }

        let finished = repo.count_finished()
            .await.expect("couldn't count the finished summaries");
        for state in BroadcastState::TERMINAL {
            assert!(finished.contains(&(state, Count::<ScheduledBroadcast>::new(1))),
                "{state} is missing from {finished:?}");
        }
    }

    #[tokio::test]
    async fn only_the_finished_rows_are_cleaned_up() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        queue(&db, chat(&db, -1001234567890).await, 0).await;
        queue(&db, chat(&db, -1009876543210).await, 0).await;
        let claimed = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        repo.finish(claimed[0].id, BroadcastState::Sent)
            .await.expect("couldn't finish the summary");

        // Nothing has been finished for long enough yet.
        let removed = repo.delete_finished(Utc::now() - Duration::from_secs(600))
            .await.expect("couldn't clean the summaries up");
        assert_eq!(removed, 0);

        let removed = repo.delete_finished(Utc::now() + Duration::from_secs(600))
            .await.expect("couldn't clean the summaries up");
        assert_eq!(removed, 1);
        let pending = repo.count_pending()
            .await.expect("couldn't count the pending summaries");
        assert_eq!(pending, 1);
    }

    /// A chat known only by its `chat_instance` can never be messaged, so a row pointing at one is
    /// given up on rather than handed to the worker — and must not come back with every tick.
    #[tokio::test]
    async fn a_summary_for_a_chat_without_a_telegram_id_is_given_up_on() {
        let db = fresh_db().await;
        let repo = ScheduledBroadcasts::new(db.clone());
        let internal_id = sqlx::query_scalar!("INSERT INTO Chats (chat_instance) VALUES ('inline-only') RETURNING id")
            .fetch_one(&db).await.expect("couldn't create the inline-only chat");
        queue(&db, internal_id, 0).await;

        let claimed = repo.claim_due(Limit::new(10), lease())
            .await.expect("couldn't claim the summaries");
        assert!(claimed.is_empty());

        let finished = repo.count_finished()
            .await.expect("couldn't count the finished summaries");
        assert_eq!(finished, vec![(BroadcastState::Failed, Count::<ScheduledBroadcast>::new(1))]);
        let claimed_again = repo.claim_due(Limit::new(10), Utc::now())
            .await.expect("couldn't claim the summaries");
        assert!(claimed_again.is_empty());
    }
}
