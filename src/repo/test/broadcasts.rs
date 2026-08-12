use std::time::Duration;
use chrono::Utc;
use sqlx::{Pool, Postgres};
use crate::repo::{BroadcastState, ScheduledBroadcasts};
use crate::domain::primitives::Limit;
use crate::domain::primitives::chat::TelegramChatId;
use crate::repo::test::{create_chat, far_future, fresh_db};

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

/// What the table says became of each summary. The repository no longer counts this — the queue's
/// history is read by a dashboard panel, not by a gauge — so the tests read it the same way.
async fn finished_states(db: &Pool<Postgres>) -> Vec<(String, i64)> {
    sqlx::query!(r#"SELECT state::text AS "state!", count(*) AS "count!" FROM Scheduled_Shrink_Broadcasts
            WHERE finished_at IS NOT NULL GROUP BY state ORDER BY 1"#)
        .fetch_all(db).await.expect("couldn't count the finished summaries")
        .into_iter().map(|row| (row.state, row.count)).collect()
}

#[tokio::test]
async fn a_queued_summary_is_claimed_with_the_chat_it_is_owed_to() {
    let db = fresh_db().await;
    let repo = ScheduledBroadcasts::new(db.clone());
    let internal_id = create_chat(&db, -1001234567890).await;
    queue(&db, internal_id, 0).await;

    let claimed = repo.claim_due(Limit::new(10), far_future()).await.expect("couldn't claim the summaries");

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
    queue(&db, create_chat(&db, -1001234567890).await, 0).await;

    let claimed = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");
    assert_eq!(claimed.len(), 1);
    let claimed_again = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");
    assert!(claimed_again.is_empty());
}

/// A worker that dies mid-batch must not take its summaries down with it — which is the whole
/// reason the queue exists.
#[tokio::test]
async fn a_summary_of_an_expired_lease_comes_back() {
    let db = fresh_db().await;
    let repo = ScheduledBroadcasts::new(db.clone());
    queue(&db, create_chat(&db, -1001234567890).await, 0).await;

    let expired = Utc::now() - Duration::from_secs(1);
    let claimed = repo.claim_due(Limit::new(10), expired)
        .await.expect("couldn't claim the summaries");
    assert_eq!(claimed.len(), 1);
    let claimed_again = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");
    assert_eq!(claimed_again.len(), 1);
    assert_eq!(claimed_again[0].id, claimed[0].id);
}

/// Two runs of the same day owe the chat one message, not two. The unique index is what says so.
#[tokio::test]
async fn the_same_chat_and_day_is_queued_once() {
    let db = fresh_db().await;
    let repo = ScheduledBroadcasts::new(db.clone());
    let internal_id = create_chat(&db, -1001234567890).await;

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
    let internal_id = create_chat(&db, -1001234567890).await;

    queue(&db, internal_id, 0).await;
    queue(&db, internal_id, 1).await;

    let pending = repo.count_pending().await.expect("couldn't count the pending summaries");
    assert_eq!(pending, 2);
}

#[tokio::test]
async fn a_postponed_summary_counts_its_attempts() {
    let db = fresh_db().await;
    let repo = ScheduledBroadcasts::new(db.clone());
    queue(&db, create_chat(&db, -1001234567890).await, 0).await;
    let claimed = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");

    let attempts = repo.postpone(claimed[0].id, Utc::now() - Duration::from_secs(1))
        .await.expect("couldn't postpone the summary");
    assert_eq!(attempts, 1);

    // The count the back-off is computed from has to survive the round trip, or every attempt
    // would rest as long as the first.
    let claimed_again = repo.claim_due(Limit::new(10), far_future())
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
    queue(&db, create_chat(&db, -1001234567890).await, 0).await;
    queue(&db, create_chat(&db, -1009876543210).await, 0).await;
    let claimed = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");
    assert_eq!(claimed.len(), 2);

    repo.finish(claimed[0].id, BroadcastState::Sent)
        .await.expect("couldn't finish the summary");
    repo.finish(claimed[1].id, BroadcastState::Unreachable)
        .await.expect("couldn't finish the summary");

    let claimed_again = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");
    assert!(claimed_again.is_empty());
    let pending = repo.count_pending()
        .await.expect("couldn't count the pending summaries");
    assert_eq!(pending, 0);

    assert_eq!(finished_states(&db).await,
        vec![("sent".to_owned(), 1), ("unreachable".to_owned(), 1)]);
}

/// Every terminal state has to survive the trip to the database and back, or a row would end up
/// counted under the wrong ending.
#[tokio::test]
async fn every_terminal_state_survives_the_round_trip() {
    let db = fresh_db().await;
    let repo = ScheduledBroadcasts::new(db.clone());
    for (i, _) in BroadcastState::TERMINAL.iter().enumerate() {
        let telegram_id = -1001234567890 - i64::try_from(i).expect("the index fits");
        queue(&db, create_chat(&db, telegram_id).await, 0).await;
    }
    let claimed = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");

    for (broadcast, state) in claimed.iter().zip(BroadcastState::TERMINAL) {
        repo.finish(broadcast.id, state)
            .await.expect("couldn't finish the summary");
    }

    let finished = finished_states(&db).await;
    for state in BroadcastState::TERMINAL {
        assert!(finished.contains(&(state.to_string(), 1)), "{state} is missing from {finished:?}");
    }
}

#[tokio::test]
async fn only_the_finished_rows_are_cleaned_up() {
    let db = fresh_db().await;
    let repo = ScheduledBroadcasts::new(db.clone());
    queue(&db, create_chat(&db, -1001234567890).await, 0).await;
    queue(&db, create_chat(&db, -1009876543210).await, 0).await;
    let claimed = repo.claim_due(Limit::new(10), far_future())
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

    let claimed = repo.claim_due(Limit::new(10), far_future())
        .await.expect("couldn't claim the summaries");
    assert!(claimed.is_empty());

    assert_eq!(finished_states(&db).await, vec![("failed".to_owned(), 1)]);
    let claimed_again = repo.claim_due(Limit::new(10), Utc::now())
        .await.expect("couldn't claim the summaries");
    assert!(claimed_again.is_empty());
}
