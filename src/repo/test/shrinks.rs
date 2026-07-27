use sqlx::{Pool, Postgres};
use crate::domain::primitives::{DaysCount, LengthChange, Ratio, UserId, Username};
use crate::domain::primitives::chat::TelegramChatId;
use crate::repo;
use crate::repo::test::{CHAT_ID, CHAT_ID_KIND, NAME, start_postgres, UID, USER_ID};

fn grace_days() -> DaysCount {
    DaysCount::new(7)
}

/// `<= 1` disables the ramp, applying the full ratio from the first overdue day — the old,
/// pre-ramp behavior that most tests want so their expected losses stay simple round numbers.
fn no_ramp() -> DaysCount {
    DaysCount::new(0)
}

/// Inserts a dick directly (bypassing `create_or_grow`) with an explicit age, so we can seed
/// dicks that look stale. A direct INSERT is required because the `Dicks` BEFORE UPDATE trigger
/// forbids touching a row that was grown today — which a freshly created dick always is.
async fn seed_aged_dick(db: &Pool<Postgres>, internal_chat_id: i64, uid: i64, length: i64, days_ago: i32) {
    sqlx::query!(
        "INSERT INTO Dicks (uid, chat_id, length, updated_at) \
            VALUES ($1, $2, $3, current_timestamp - make_interval(days => $4))",
        uid, internal_chat_id, length, days_ago)
        .execute(db)
        .await
        .expect("couldn't seed an aged dick");
}

async fn seed_aged_dick_with_bonus_attempts(
    db: &Pool<Postgres>,
    internal_chat_id: i64,
    uid: i64,
    length: i64,
    days_ago: i32,
    bonus_attempts: i32,
) {
    sqlx::query!(
        "INSERT INTO Dicks (uid, chat_id, length, updated_at, bonus_attempts) \
            VALUES ($1, $2, $3, current_timestamp - make_interval(days => $4), $5)",
        uid, internal_chat_id, length, days_ago, bonus_attempts + 1)
        .execute(db)
        .await
        .expect("couldn't seed an aged dick with a bonus attempt");
}

async fn bonus_attempts_of(db: &Pool<Postgres>, uid: i64, internal_chat_id: i64) -> i32 {
    sqlx::query_scalar!("SELECT bonus_attempts FROM Dicks WHERE uid = $1 AND chat_id = $2", uid, internal_chat_id)
        .fetch_one(db)
        .await
        .expect("couldn't read bonus_attempts")
}

async fn logged_shrinks_count(db: &Pool<Postgres>, uid: i64) -> i64 {
    sqlx::query_scalar!("SELECT count(*) AS \"c!\" FROM Stale_Dick_Shrinks WHERE uid = $1", uid)
        .fetch_one(db)
        .await
        .expect("couldn't count shrinks")
}

async fn seed_old_shrink(db: &Pool<Postgres>, internal_chat_id: i64, uid: i64, lost_length: i64, days_ago: i32) {
    sqlx::query!(
        "INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length, created_at) \
            VALUES ($1, $2, $3, current_date - $4::int)",
        internal_chat_id, uid, lost_length, days_ago)
        .execute(db)
        .await
        .expect("couldn't seed an old shrink");
}

async fn internal_chat_id(db: &Pool<Postgres>) -> i64 {
    sqlx::query_scalar!("SELECT id FROM Chats WHERE chat_id = $1", CHAT_ID)
        .fetch_one(db)
        .await
        .expect("couldn't resolve the internal chat id")
}

async fn updated_at_of(db: &Pool<Postgres>, uid: i64, internal_chat_id: i64) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar!("SELECT updated_at FROM Dicks WHERE uid = $1 AND chat_id = $2", uid, internal_chat_id)
        .fetch_one(db)
        .await
        .expect("couldn't read updated_at")
}

async fn length_of(db: &Pool<Postgres>, uid: i64, internal_chat_id: i64) -> i64 {
    sqlx::query_scalar!("SELECT length FROM Dicks WHERE uid = $1 AND chat_id = $2", uid, internal_chat_id)
        .fetch_one(db)
        .await
        .expect("couldn't read length")
}

#[tokio::test]
async fn test_perform_daily_shrink() {
    let (_container, db) = start_postgres().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    // A fresh dick for USER_ID — also creates the Chats row. It must NOT be shrunk (grown today).
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(100))
        .await.expect("couldn't create the fresh dick");
    let chat_id = internal_chat_id(&db).await;

    // A stale, positive dick — the only expected victim.
    let victim_uid = UID + 1;
    users.create_or_update(UserId::literal(victim_uid), "stale-victim")
        .await.expect("couldn't create the victim user");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;
    let victim_updated_at_before = updated_at_of(&db, victim_uid, chat_id).await;

    // A stale but zero-length dick — must be left alone (only length > 0 shrinks).
    let zero_uid = UID + 2;
    users.create_or_update(UserId::literal(zero_uid), "zero-length")
        .await.expect("couldn't create the zero-length user");
    seed_aged_dick(&db, chat_id, zero_uid, 0, 10).await;

    // A stale, positive dick with a pending bonus attempt — shrinking it must not silently burn
    // the bonus attempt (the Dicks trigger decrements it on every update unless counteracted).
    let bonus_uid = UID + 3;
    users.create_or_update(UserId::literal(bonus_uid), "has-bonus")
        .await.expect("couldn't create the bonus user");
    seed_aged_dick_with_bonus_attempts(&db, chat_id, bonus_uid, 100, 10, 1).await;

    let events = shrinks.perform_daily_shrink(Ratio::literal(0.1), grace_days(), no_ramp())
        .await.expect("couldn't perform the daily shrink");

    assert_eq!(events.len(), 2, "the two stale, positive dicks should have shrunk");
    let event = events.iter().find(|e| e.uid == UserId::literal(victim_uid))
        .expect("the victim's shrink event is missing");
    assert_eq!(event.owner_name, Username::from("stale-victim"));
    assert_eq!(event.lost_length, 10, "loss = ceil(100 * 0.1)");
    assert_eq!(event.new_length, 90);
    assert_eq!(event.messageable_chat_id, Some(TelegramChatId::new(CHAT_ID)));

    // The fresh and the zero-length dicks are untouched.
    assert_eq!(length_of(&db, UID, chat_id).await, 100, "the fresh dick must not shrink");
    assert_eq!(length_of(&db, zero_uid, chat_id).await, 0, "the zero-length dick must not shrink");

    // Exactly one shrink was logged, and the shrink didn't bump updated_at (so it stays repeatable).
    assert_eq!(logged_shrinks_count(&db, victim_uid).await, 1);
    let victim_updated_at_after = updated_at_of(&db, victim_uid, chat_id).await;
    assert_eq!(victim_updated_at_before, victim_updated_at_after, "shrinking must not touch updated_at");

    // The pending bonus attempt survives the shrink instead of being silently burned by the trigger.
    assert_eq!(bonus_attempts_of(&db, bonus_uid, chat_id).await, 1, "shrinking must not consume a bonus attempt");
}

#[tokio::test]
async fn test_perform_daily_shrink_ramps_up_the_ratio() {
    let (_container, db) = start_postgres().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create a dummy dick and the Chats row");
    let chat_id = internal_chat_id(&db).await;

    // Just past the grace period (7 days): the first overdue day, so the ramp (over 4 days)
    // is at its smallest step — 1/4 of the full ratio.
    let just_overdue_uid = UID + 1;
    users.create_or_update(UserId::literal(just_overdue_uid), "just-overdue")
        .await.expect("couldn't create the just-overdue user");
    seed_aged_dick(&db, chat_id, just_overdue_uid, 1000, 7).await;

    // Overdue for exactly the ramp's length (grace + ramp = 11 days): the ramp has fully
    // kicked in, so the full ratio applies.
    let fully_ramped_uid = UID + 2;
    users.create_or_update(UserId::literal(fully_ramped_uid), "fully-ramped")
        .await.expect("couldn't create the fully-ramped user");
    seed_aged_dick(&db, chat_id, fully_ramped_uid, 1000, 10).await;

    // Way overdue: the ramp is capped, so this must lose the same as the fully-ramped dick above,
    // not more.
    let way_overdue_uid = UID + 3;
    users.create_or_update(UserId::literal(way_overdue_uid), "way-overdue")
        .await.expect("couldn't create the way-overdue user");
    seed_aged_dick(&db, chat_id, way_overdue_uid, 1000, 20).await;

    let events = shrinks.perform_daily_shrink(Ratio::literal(0.5), grace_days(), DaysCount::new(4))
        .await.expect("couldn't perform the daily shrink");

    let loss_of = |uid: i64| events.iter()
        .find(|e| e.uid == UserId::literal(uid))
        .unwrap_or_else(|| panic!("no shrink event for uid {uid}"))
        .lost_length;
    assert_eq!(loss_of(just_overdue_uid), 125, "1/4 of the ramp: ceil(1000 * 0.5 * 1/4)");
    assert_eq!(loss_of(fully_ramped_uid), 500, "ramp fully kicked in: ceil(1000 * 0.5)");
    assert_eq!(loss_of(way_overdue_uid), 500, "ramp is capped at the full ratio, not exceeded");
}

#[tokio::test]
async fn test_get_recent_shrinks_respects_the_day_window() {
    let (_container, db) = start_postgres().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(100))
        .await.expect("couldn't create the fresh dick");
    let chat_id = internal_chat_id(&db).await;

    let victim_uid = UID + 1;
    users.create_or_update(UserId::literal(victim_uid), "victim")
        .await.expect("couldn't create the victim");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;

    // Today's shrink (logged with created_at = current_date).
    shrinks.perform_daily_shrink(Ratio::literal(0.1), grace_days(), no_ramp())
        .await.expect("couldn't perform the daily shrink");
    // An older shrink, 8 days ago — outside a 7-day window.
    seed_old_shrink(&db, chat_id, UID, 5, 8).await;

    let recent = shrinks.get_recent_shrinks(&CHAT_ID_KIND, DaysCount::new(7)).await
        .expect("couldn't fetch recent shrinks");
    assert_eq!(recent.len(), 1, "only the shrink within the last 7 days should be returned");
    assert_eq!(recent[0].lost_length, 10);
}

#[tokio::test]
async fn test_get_player_uids() {
    let (_container, db) = start_postgres().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create the first dick");
    let chat_id = internal_chat_id(&db).await;

    for n in 1..=2 {
        let uid = UID + n;
        users.create_or_update(UserId::literal(uid), &format!("player-{n}"))
            .await.expect("couldn't create a player");
        seed_aged_dick(&db, chat_id, uid, 10, 1).await;
    }

    let mut uids = dicks.get_player_uids(&CHAT_ID_KIND).await
        .expect("couldn't fetch player uids");
    uids.sort_by_key(|u| u.value());
    assert_eq!(uids, vec![
        USER_ID,
        UserId::literal(UID + 1),
        UserId::literal(UID + 2),
    ]);
}
