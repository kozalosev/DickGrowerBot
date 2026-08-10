use autometrics::autometrics;
use anyhow::Context;
use chrono::{DateTime, Utc};
use itertools::Itertools;
use crate::config::MessageGroup;
use crate::domain::primitives::{AttemptsCount, Count, LanguageCode, Limit, ScheduledDeletionId};
use crate::domain::primitives::chat::{InlineMessageId, TelegramChatId, TelegramMessageId};
use crate::repository;

/// What the row is about: the bot's own answer, the command that caused it, or an inline message.
/// A command is told apart from an answer because only it can be refused by Telegram for a reason
/// worth remembering — the bot not being an administrator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, strum_macros::Display, strum_macros::EnumIter)]
#[strum(serialize_all = "lowercase")]
#[sqlx(type_name = "message_kind", rename_all = "lowercase")]
pub enum MessageKind {
    Reply,
    Command,
    Inline,
}

/// How far a row got. `Created` and `Warned` are the actionable ones — a message that already
/// carries the warning has only its deletion left, and an inline message stays `Created` to its
/// end, as a warning would tell nothing its placeholder doesn't.
///
/// The rest are terminal and stay in the table until the cleaning process removes them, so that a
/// queue which isn't doing its job can be read rather than guessed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "deletion_state", rename_all = "snake_case")]
pub enum DeletionState {
    Created,
    Warned,
    /// The bot deleted the message, or replaced it with its placeholder.
    Removed,
    /// The message was already gone when its turn came: someone deleted it first.
    RemovedBefore,
    /// The message grew older than Telegram lets a bot delete, so it was never even tried.
    Expired,
    /// Every attempt failed for a reason that looked transient and never stopped being one.
    Failed,
}

impl DeletionState {
    /// The states a row never leaves. What is kept in the table until the cleaning process runs,
    /// and what the gauges of [`crate::metrics::SELF_DESTRUCTION_FINISHED`] are split by.
    pub const TERMINAL: [Self; 4] = [Self::Removed, Self::RemovedBefore, Self::Expired, Self::Failed];
}

/// The message a row points at. The two variants are the two things Telegram lets us address, and
/// they need different calls: a chat message is deleted, an inline one can only be edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeletionTarget {
    ChatMessage {
        chat_id: TelegramChatId,
        message_id: TelegramMessageId,
    },
    InlineMessage(InlineMessageId),
}

/// A message waiting for its self-destruction, as it is written.
#[derive(Clone, Debug)]
pub struct NewDeletion {
    pub target: DeletionTarget,
    pub kind: MessageKind,
    pub group: MessageGroup,
    pub lang_code: LanguageCode,
    pub fire_after: DateTime<Utc>,
}

/// A row claimed by the worker.
#[derive(Clone, Debug)]
pub struct ScheduledDeletion {
    pub id: ScheduledDeletionId,
    pub target: DeletionTarget,
    pub kind: MessageKind,
    pub group: MessageGroup,
    pub lang_code: LanguageCode,
    pub state: DeletionState,
    /// When the message was scheduled, which is when it was sent — its age.
    pub created_at: DateTime<Utc>,
    /// Attempts that have already failed, which is what the back-off is computed from.
    pub attempts: AttemptsCount,
}

impl DeletionTarget {
    /// The three columns a target is stored in, in the order [`ScheduledDeletions::schedule`]
    /// binds them.
    fn columns(&self) -> (Option<TelegramChatId>, Option<TelegramMessageId>, Option<InlineMessageId>) {
        match self {
            Self::ChatMessage { chat_id, message_id } => (Some(*chat_id), Some(*message_id), None),
            Self::InlineMessage(id) => (None, None, Some(id.clone())),
        }
    }

    /// The way back. `None` means the row addresses no message at all: the table's CHECK constraint
    /// rules that out, so it takes a hand-edited row to produce one.
    fn of_columns(
        chat_id: Option<TelegramChatId>,
        message_id: Option<TelegramMessageId>,
        inline_message_id: Option<InlineMessageId>,
    ) -> Option<Self> {
        match (chat_id, message_id, inline_message_id) {
            (Some(chat_id), Some(message_id), _) => Some(Self::ChatMessage { chat_id, message_id }),
            (_, _, Some(inline_message_id)) => Some(Self::InlineMessage(inline_message_id)),
            _ => None,
        }
    }
}

repository!(ScheduledDeletions,
    /// Writes the messages down as candidates for deletion.
    ///
    /// The same message can arrive here more than once: paging an inline leaderboard rewrites that
    /// message and schedules it again. `ON CONFLICT DO NOTHING` keeps the moment it was given the
    /// first time, so each tap on a button doesn't move its deletion further away.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(count = deletions.len()))]
    pub async fn schedule(&self, deletions: &[NewDeletion]) -> anyhow::Result<()> {
        if deletions.is_empty() {
            return Ok(())
        }
        let (chat_ids, message_ids, inline_message_ids, kinds, groups, lang_codes, fire_afters):
            (Vec<_>, Vec<_>, Vec<_>, Vec<_>, Vec<_>, Vec<_>, Vec<_>) = deletions.iter()
            .map(|deletion| {
                let (chat_id, message_id, inline_message_id) = deletion.target.columns();
                (chat_id, message_id, inline_message_id,
                 deletion.kind, deletion.group, deletion.lang_code.clone(), deletion.fire_after)
            })
            .multiunzip();

        sqlx::query!(
            "INSERT INTO Scheduled_Message_Deletions (chat_id, message_id, inline_message_id,
                                                      message_kind, message_group, lang_code, fire_after)
                SELECT * FROM UNNEST($1::bigint[], $2::int[], $3::text[],
                                     $4::message_kind[], $5::message_group[], $6::text[], $7::timestamptz[])
                ON CONFLICT DO NOTHING",
            &chat_ids as &[Option<TelegramChatId>],
            &message_ids as &[Option<TelegramMessageId>],
            &inline_message_ids as &[Option<InlineMessageId>],
            &kinds as &[MessageKind],
            &groups as &[MessageGroup],
            &lang_codes as &[LanguageCode],
            &fire_afters
        )
            .execute(&self.pool)
            .await
            .context("couldn't schedule the deletion of the messages")?;
        Ok(())
    },

    /// Takes up to `limit` messages whose time has come, leasing them until `lease_until`.
    ///
    /// The lease is what makes the claim exclusive: the row's lock lives only as long as this one
    /// statement, so pushing `fire_after` forward is the only thing that keeps a second instance of
    /// the bot (or an overlapping tick of the same one) from acting on the same message while it is
    /// being worked on. A worker that dies mid-batch leaves its messages to be claimed again once
    /// the lease runs out, rather than for ever.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(limit = %limit))]
    pub async fn claim_due(&self, limit: Limit, lease_until: DateTime<Utc>) -> anyhow::Result<Vec<ScheduledDeletion>> {
        let rows = sqlx::query!(
            r#"UPDATE Scheduled_Message_Deletions SET fire_after = $2
                WHERE id IN (
                    SELECT id FROM Scheduled_Message_Deletions
                    WHERE fire_after <= current_timestamp AND finished_at IS NULL
                    ORDER BY fire_after
                    LIMIT $1
                    FOR UPDATE SKIP LOCKED
                )
                RETURNING id AS "id: ScheduledDeletionId", chat_id AS "chat_id: TelegramChatId",
                          message_id AS "message_id: TelegramMessageId",
                          inline_message_id AS "inline_message_id: InlineMessageId",
                          message_kind AS "kind: MessageKind", message_group AS "group: MessageGroup",
                          lang_code AS "lang_code: LanguageCode", state AS "state: DeletionState",
                          created_at, attempts AS "attempts!: AttemptsCount""#,
            limit as Limit, lease_until
        )
            .fetch_all(&self.pool)
            .await
            .context("couldn't claim the messages due for deletion")?;

        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(target) = DeletionTarget::of_columns(row.chat_id, row.message_id, row.inline_message_id) else {
                // Left as failed rather than skipped: the lease runs out, and a row nobody can act
                // on would come back with every tick for ever.
                tracing::error!(id = %row.id, "a scheduled deletion points at no message, giving up on it");
                self.finish(row.id, DeletionState::Failed).await
                    .unwrap_or_else(|e| tracing::error!(id = %row.id, error = format!("{e:#}"),
                        "couldn't give up on the unusable scheduled deletion"));
                continue
            };
            claimed.push(ScheduledDeletion {
                id: row.id,
                target,
                kind: row.kind,
                group: row.group,
                lang_code: row.lang_code,
                state: row.state,
                created_at: row.created_at,
                attempts: row.attempts,
            });
        }
        Ok(claimed)
    },

    /// How many messages are still waiting — reported as a gauge, so a queue that stops draining is
    /// visible before the chats are. The finished rows are left out: they are history, and counting
    /// them would make the gauge grow on its own until the cleaning process runs.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn count_pending(&self) -> anyhow::Result<Count<ScheduledDeletion>> {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: Count<ScheduledDeletion>"
                FROM Scheduled_Message_Deletions WHERE finished_at IS NULL"#)
            .fetch_one(&self.pool)
            .await
            .context("couldn't count the pending deletions")
    },

    /// How many rows are finished but not yet cleaned, by state — the queue's own history, and the
    /// first thing to look at when messages stop disappearing.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn count_finished(&self) -> anyhow::Result<Vec<(DeletionState, Count<ScheduledDeletion>)>> {
        let rows = sqlx::query!(
            r#"SELECT state AS "state: DeletionState", count(*) AS "count!: Count<ScheduledDeletion>"
                FROM Scheduled_Message_Deletions WHERE finished_at IS NOT NULL GROUP BY state"#)
            .fetch_all(&self.pool)
            .await
            .context("couldn't count the finished deletions")?;
        Ok(rows.into_iter().map(|row| (row.state, row.count)).collect())
    },

    /// Records that the message now carries the warning and moves it to the end of the grace period.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(id = %id))]
    pub async fn mark_warned(&self, id: ScheduledDeletionId, fire_after: DateTime<Utc>) -> anyhow::Result<()> {
        sqlx::query!("UPDATE Scheduled_Message_Deletions SET state = 'warned', fire_after = $2 WHERE id = $1",
                id as ScheduledDeletionId, fire_after)
            .execute(&self.pool)
            .await
            .context("couldn't mark the scheduled deletion as warned")?;
        Ok(())
    },

    /// Counts one failed attempt and pushes the row back by `retry_after`.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(id = %id))]
    pub async fn postpone(&self, id: ScheduledDeletionId, retry_after: DateTime<Utc>) -> anyhow::Result<AttemptsCount> {
        let attempts = sqlx::query_scalar!(
            r#"UPDATE Scheduled_Message_Deletions SET attempts = attempts + 1, fire_after = $2
                    WHERE id = $1 RETURNING attempts AS "attempts!: AttemptsCount""#,
                id as ScheduledDeletionId, retry_after)
            .fetch_one(&self.pool)
            .await
            .context("couldn't postpone the scheduled deletion")?;
        Ok(attempts)
    },

    /// Leaves the row behind in a terminal state instead of dropping it, so that what the worker
    /// did — and what it couldn't do — can be read out of the table until the cleaning process
    /// takes it away.
    #[autometrics]
    #[tracing::instrument(skip_all, fields(id = %id, state = %state))]
    pub async fn finish(&self, id: ScheduledDeletionId, state: DeletionState) -> anyhow::Result<()> {
        sqlx::query!(
            "UPDATE Scheduled_Message_Deletions SET state = $2, finished_at = current_timestamp WHERE id = $1",
                id as ScheduledDeletionId, state as DeletionState)
            .execute(&self.pool)
            .await
            .context("couldn't finish the scheduled deletion")?;
        Ok(())
    },

    /// Removes the rows that were finished before `older_than`, and says how many went.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn delete_finished(&self, older_than: DateTime<Utc>) -> anyhow::Result<u64> {
        let result = sqlx::query!(
            "DELETE FROM Scheduled_Message_Deletions WHERE finished_at IS NOT NULL AND finished_at < $1",
                older_than)
            .execute(&self.pool)
            .await
            .context("couldn't clean the finished deletions up")?;
        Ok(result.rows_affected())
    },

    /// Calls the self-destruction off — for a message that has just become worth keeping, like an
    /// application answered before it could expire.
    #[autometrics]
    #[tracing::instrument(skip_all)]
    pub async fn cancel(&self, target: &DeletionTarget) -> anyhow::Result<()> {
        match target {
            DeletionTarget::ChatMessage { chat_id, message_id } => {
                sqlx::query!("DELETE FROM Scheduled_Message_Deletions WHERE chat_id = $1 AND message_id = $2",
                        chat_id as &TelegramChatId, message_id as &TelegramMessageId)
                    .execute(&self.pool)
                    .await
                    .context("couldn't cancel the deletion of a chat message")?;
            },
            DeletionTarget::InlineMessage(id) => {
                sqlx::query!("DELETE FROM Scheduled_Message_Deletions WHERE inline_message_id = $1",
                        id as &InlineMessageId)
                    .execute(&self.pool)
                    .await
                    .context("couldn't cancel the deletion of an inline message")?;
            },
        };
        Ok(())
    }
);

#[cfg(test)]
mod tests {
    use crate::literal;
    use std::time::Duration;
    use domain_types::traits::SaturatingInto;
    use chrono::Utc;
    use super::*;
    use crate::repo::test::fresh_db;

    const CHAT_ID: i64 = -1001234567890;

    fn chat_message(message_id: u32) -> DeletionTarget {
        DeletionTarget::ChatMessage {
            chat_id: TelegramChatId::new(CHAT_ID),
            message_id: TelegramMessageId::new(message_id),
        }
    }

    /// A lease long enough that nothing under test can outlive it.
    fn lease() -> DateTime<Utc> {
        Utc::now() + Duration::from_secs(600)
    }

    fn due(target: DeletionTarget, kind: MessageKind) -> NewDeletion {
        NewDeletion {
            target,
            kind,
            group: MessageGroup::Notice,
            lang_code: LanguageCode::new("en".to_owned()),
            fire_after: Utc::now() - Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn only_the_due_messages_are_claimed() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);

        let later = NewDeletion { fire_after: Utc::now() + Duration::from_secs(600), ..due(chat_message(2), MessageKind::Reply) };
        repo.schedule(&[due(chat_message(1), MessageKind::Reply), later])
            .await.expect("couldn't schedule the deletions");

        let claimed = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].target, chat_message(1));
        assert_eq!(claimed[0].kind, MessageKind::Reply);
        assert_eq!(claimed[0].state, DeletionState::Created);

        let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
        assert_eq!(pending, 2);
    }

    /// The whole point of the lease: a message being worked on is invisible to everyone else, and
    /// nothing here holds a lock long enough to do that on its own.
    #[tokio::test]
    async fn a_claimed_message_is_not_claimed_again() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion");

        let claimed = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert_eq!(claimed.len(), 1);
        let claimed_again = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert!(claimed_again.is_empty());
    }

    /// A worker that dies mid-batch must not take its messages down with it.
    #[tokio::test]
    async fn a_message_of_an_expired_lease_comes_back() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion");

        let expired = Utc::now() - Duration::from_secs(1);
        let claimed = repo.claim_due(literal!(Limit = 10), expired).await.expect("couldn't claim the deletions");
        assert_eq!(claimed.len(), 1);
        let claimed_again = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert_eq!(claimed_again.len(), 1);
        assert_eq!(claimed_again[0].id, claimed[0].id);
    }

    #[tokio::test]
    async fn an_inline_message_survives_the_round_trip() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        let target = DeletionTarget::InlineMessage(InlineMessageId::new("AgAAAOEcAABzXwsRJ0Cs2A".to_owned()));

        repo.schedule(&[due(target.clone(), MessageKind::Inline)])
            .await.expect("couldn't schedule the deletion");

        let claimed = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].target, target);
        assert_eq!(claimed[0].kind, MessageKind::Inline);
    }

    #[tokio::test]
    async fn the_same_message_is_scheduled_once() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);

        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion");
        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion again");

        let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
        assert_eq!(pending, 1);
    }

    #[tokio::test]
    async fn a_warned_message_comes_back_at_the_end_of_the_grace_period() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion");
        let claimed = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");

        repo.mark_warned(claimed[0].id, Utc::now() + Duration::from_secs(600))
            .await.expect("couldn't mark the deletion as warned");
        let claimed_again = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert!(claimed_again.is_empty());

        repo.mark_warned(claimed[0].id, Utc::now() - Duration::from_secs(1))
            .await.expect("couldn't mark the deletion as warned");
        let claimed_again = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert_eq!(claimed_again.len(), 1);
        assert_eq!(claimed_again[0].state, DeletionState::Warned);
    }

    #[tokio::test]
    async fn a_postponed_message_counts_its_attempts() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion");
        let claimed = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");

        let attempts = repo.postpone(claimed[0].id, Utc::now() - Duration::from_secs(1))
            .await.expect("couldn't postpone the deletion");
        assert_eq!(attempts, 1);
        let attempts = repo.postpone(claimed[0].id, Utc::now() - Duration::from_secs(1))
            .await.expect("couldn't postpone the deletion again");
        assert_eq!(attempts, 2);

        // The count the back-off is computed from has to survive the round trip, or every attempt
        // would rest as long as the first.
        let claimed_again = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert_eq!(claimed_again.len(), 1);
        assert_eq!(claimed_again[0].attempts, 2);
    }

    /// A finished row stays in the table as the account of what happened, but is out of the
    /// worker's way and out of the gauge that says how much work is left.
    #[tokio::test]
    async fn a_finished_message_is_kept_but_never_claimed() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply), due(chat_message(2), MessageKind::Command)])
            .await.expect("couldn't schedule the deletions");
        let claimed = repo.claim_due(literal!(Limit = 10), Utc::now() - Duration::from_secs(1))
            .await.expect("couldn't claim the deletions");
        assert_eq!(claimed.len(), 2);

        repo.finish(claimed[0].id, DeletionState::Removed).await.expect("couldn't finish the deletion");
        repo.finish(claimed[1].id, DeletionState::Failed).await.expect("couldn't fail the deletion");

        let claimed_again = repo.claim_due(literal!(Limit = 10), lease()).await.expect("couldn't claim the deletions");
        assert!(claimed_again.is_empty());
        let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
        assert_eq!(pending, 0);

        let mut finished = repo.count_finished().await.expect("couldn't count the finished deletions");
        finished.sort_by_key(|(state, _)| state.to_string());
        assert_eq!(finished, vec![(DeletionState::Failed, literal!(Count<ScheduledDeletion> = 1)), (DeletionState::Removed, literal!(Count<ScheduledDeletion> = 1))]);
    }

    /// Every terminal state has to survive the trip to the database and back, or a row would end up
    /// counted under the wrong ending — the two-word one especially, as it is the only place the
    /// snake_case spelling of the enum matters.
    #[tokio::test]
    async fn every_terminal_state_survives_the_round_trip() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        let messages: Vec<_> = (1..=DeletionState::TERMINAL.len())
            .map(|i| due(chat_message(i.saturating_into()), MessageKind::Reply))
            .collect();
        repo.schedule(&messages).await.expect("couldn't schedule the deletions");
        let claimed = repo.claim_due(literal!(Limit = 10), lease())
            .await.expect("couldn't claim the deletions");

        for (deletion, state) in claimed.iter().zip(DeletionState::TERMINAL) {
            repo.finish(deletion.id, state).await.expect("couldn't finish the deletion");
        }

        let finished = repo.count_finished().await.expect("couldn't count the finished deletions");
        for state in DeletionState::TERMINAL {
            assert!(finished.contains(&(state, literal!(Count<ScheduledDeletion> = 1))), "{state} is missing from {finished:?}");
        }
    }

    #[tokio::test]
    async fn only_the_finished_rows_are_cleaned_up() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply), due(chat_message(2), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletions");
        let claimed = repo.claim_due(literal!(Limit = 10), lease())
            .await.expect("couldn't claim the deletions");
        repo.finish(claimed[0].id, DeletionState::Failed)
            .await.expect("couldn't fail the deletion");

        // Nothing has been finished for long enough yet.
        let removed = repo.delete_finished(Utc::now() - Duration::from_secs(600))
            .await.expect("couldn't clean the deletions up");
        assert_eq!(removed, 0);

        let removed = repo.delete_finished(Utc::now() + Duration::from_secs(600))
            .await.expect("couldn't clean the deletions up");
        assert_eq!(removed, 1);
        let pending = repo.count_pending()
            .await.expect("couldn't count the pending deletions");
        assert_eq!(pending, 1);
    }

    /// A row addressing no message can only be made by hand, around the CHECK constraint that
    /// forbids it. It must not be handed to the worker as some empty id it would then spend three
    /// attempts on — and it must not keep coming back with every tick either.
    #[tokio::test]
    async fn a_row_that_addresses_no_message_is_given_up_on() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db.clone());
        sqlx::query!("ALTER TABLE Scheduled_Message_Deletions DROP CONSTRAINT scheduled_message_deletions_check")
            .execute(&db).await.expect("couldn't drop the check constraint");
        sqlx::query!("INSERT INTO Scheduled_Message_Deletions (message_kind, message_group, lang_code, fire_after)
                VALUES ('reply', 'notice', 'en', current_timestamp - interval '1 minute')")
            .execute(&db).await.expect("couldn't insert the malformed row");

        let claimed = repo.claim_due(literal!(Limit = 10), lease())
            .await.expect("couldn't claim the deletions");
        assert!(claimed.is_empty());

        let finished = repo.count_finished()
            .await.expect("couldn't count the finished deletions");
        assert_eq!(finished, vec![(DeletionState::Failed, literal!(Count<ScheduledDeletion> = 1))]);
        let claimed_again = repo.claim_due(literal!(Limit = 10), Utc::now())
            .await.expect("couldn't claim the deletions");
        assert!(claimed_again.is_empty());
    }

    /// A removed message is history like any other ending, so its row waits for the cleaning
    /// process rather than disappearing with it.
    #[tokio::test]
    async fn a_removed_message_stays_until_it_is_cleaned_up() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
            .await.expect("couldn't schedule the deletion");
        let claimed = repo.claim_due(literal!(Limit = 10), lease())
            .await.expect("couldn't claim the deletions");

        repo.finish(claimed[0].id, DeletionState::Removed)
            .await.expect("couldn't finish the deletion");

        let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
        assert_eq!(pending, 0);
        let finished = repo.count_finished().await.expect("couldn't count the finished deletions");
        assert_eq!(finished, vec![(DeletionState::Removed, literal!(Count<ScheduledDeletion> = 1))]);

        let removed = repo.delete_finished(Utc::now() + Duration::from_secs(600))
            .await.expect("couldn't clean the deletions up");
        assert_eq!(removed, 1);
    }

    #[tokio::test]
    async fn a_cancelled_message_is_kept() {
        let db = fresh_db().await;
        let repo = ScheduledDeletions::new(db);
        let inline = DeletionTarget::InlineMessage(InlineMessageId::new("AgAAAOEcAABzXwsRJ0Cs2A".to_owned()));
        repo.schedule(&[due(chat_message(1), MessageKind::Reply), due(inline.clone(), MessageKind::Inline)])
            .await.expect("couldn't schedule the deletions");

        repo.cancel(&chat_message(1)).await.expect("couldn't cancel the deletion");
        repo.cancel(&inline).await.expect("couldn't cancel the deletion");

        let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
        assert_eq!(pending, 0);
    }
}
