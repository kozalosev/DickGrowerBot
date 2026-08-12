use std::time::Duration;
use domain_types::traits::SaturatingInto;
use chrono::{DateTime, Utc};
use crate::config::MessageGroup;
use crate::domain::primitives::{Count, LanguageCode, Limit};
use crate::domain::primitives::chat::{InlineMessageId, TelegramChatId, TelegramMessageId};
use crate::repo::{DeletionState, DeletionTarget, MessageKind, NewDeletion, ScheduledDeletion, ScheduledDeletions};
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

    let claimed = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
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

    let claimed = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
    assert_eq!(claimed.len(), 1);
    let claimed_again = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
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
    let claimed = repo.claim_due(Limit::new(10), expired).await.expect("couldn't claim the deletions");
    assert_eq!(claimed.len(), 1);
    let claimed_again = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
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

    let claimed = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
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
    let claimed = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");

    repo.mark_warned(claimed[0].id, Utc::now() + Duration::from_secs(600))
        .await.expect("couldn't mark the deletion as warned");
    let claimed_again = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
    assert!(claimed_again.is_empty());

    repo.mark_warned(claimed[0].id, Utc::now() - Duration::from_secs(1))
        .await.expect("couldn't mark the deletion as warned");
    let claimed_again = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
    assert_eq!(claimed_again.len(), 1);
    assert_eq!(claimed_again[0].state, DeletionState::Warned);
}

#[tokio::test]
async fn a_postponed_message_counts_its_attempts() {
    let db = fresh_db().await;
    let repo = ScheduledDeletions::new(db);
    repo.schedule(&[due(chat_message(1), MessageKind::Reply)])
        .await.expect("couldn't schedule the deletion");
    let claimed = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");

    let attempts = repo.postpone(claimed[0].id, Utc::now() - Duration::from_secs(1))
        .await.expect("couldn't postpone the deletion");
    assert_eq!(attempts, 1);
    let attempts = repo.postpone(claimed[0].id, Utc::now() - Duration::from_secs(1))
        .await.expect("couldn't postpone the deletion again");
    assert_eq!(attempts, 2);

    // The count the back-off is computed from has to survive the round trip, or every attempt
    // would rest as long as the first.
    let claimed_again = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
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
    let claimed = repo.claim_due(Limit::new(10), Utc::now() - Duration::from_secs(1))
        .await.expect("couldn't claim the deletions");
    assert_eq!(claimed.len(), 2);

    repo.finish(claimed[0].id, DeletionState::Removed).await.expect("couldn't finish the deletion");
    repo.finish(claimed[1].id, DeletionState::Failed).await.expect("couldn't fail the deletion");

    let claimed_again = repo.claim_due(Limit::new(10), lease()).await.expect("couldn't claim the deletions");
    assert!(claimed_again.is_empty());
    let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
    assert_eq!(pending, 0);

    let mut finished = repo.count_finished().await.expect("couldn't count the finished deletions");
    finished.sort_by_key(|(state, _)| state.to_string());
    assert_eq!(finished, vec![(DeletionState::Failed, Count::<ScheduledDeletion>::new(1)), (DeletionState::Removed, Count::<ScheduledDeletion>::new(1))]);
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
    let claimed = repo.claim_due(Limit::new(10), lease())
        .await.expect("couldn't claim the deletions");

    for (deletion, state) in claimed.iter().zip(DeletionState::TERMINAL) {
        repo.finish(deletion.id, state).await.expect("couldn't finish the deletion");
    }

    let finished = repo.count_finished().await.expect("couldn't count the finished deletions");
    for state in DeletionState::TERMINAL {
        assert!(finished.contains(&(state, Count::<ScheduledDeletion>::new(1))), "{state} is missing from {finished:?}");
    }
}

#[tokio::test]
async fn only_the_finished_rows_are_cleaned_up() {
    let db = fresh_db().await;
    let repo = ScheduledDeletions::new(db);
    repo.schedule(&[due(chat_message(1), MessageKind::Reply), due(chat_message(2), MessageKind::Reply)])
        .await.expect("couldn't schedule the deletions");
    let claimed = repo.claim_due(Limit::new(10), lease())
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

    let claimed = repo.claim_due(Limit::new(10), lease())
        .await.expect("couldn't claim the deletions");
    assert!(claimed.is_empty());

    let finished = repo.count_finished()
        .await.expect("couldn't count the finished deletions");
    assert_eq!(finished, vec![(DeletionState::Failed, Count::<ScheduledDeletion>::new(1))]);
    let claimed_again = repo.claim_due(Limit::new(10), Utc::now())
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
    let claimed = repo.claim_due(Limit::new(10), lease())
        .await.expect("couldn't claim the deletions");

    repo.finish(claimed[0].id, DeletionState::Removed)
        .await.expect("couldn't finish the deletion");

    let pending = repo.count_pending().await.expect("couldn't count the pending deletions");
    assert_eq!(pending, 0);
    let finished = repo.count_finished().await.expect("couldn't count the finished deletions");
    assert_eq!(finished, vec![(DeletionState::Removed, Count::<ScheduledDeletion>::new(1))]);

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
