use sqlx::{Pool, Postgres};
use crate::domain::primitives::{DaysCount, LengthChange, Limit, Offset, Ratio};
use crate::domain::primitives::chat::{InternalChatId, TelegramChatId};
use crate::repo;
use crate::repo::test::{user_id, CHAT_ID, CHAT_ID_KIND, NAME, fresh_db, UID, USER_ID};
use domain_types::literal;

const GRACE_DAYS: DaysCount = DaysCount::new(7);

/// `<= 1` disables the ramp, applying the full ratio from the first overdue day — the old,
/// pre-ramp behavior that most tests want so their expected losses stay simple round numbers.
const NO_RAMP: DaysCount = DaysCount::new(0);

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

/// What today's run took off that dick. The shrink no longer hands the events back, so this is
/// where a loss is read from — the same table the summary is rendered out of.
async fn lost_length_of(db: &Pool<Postgres>, uid: i64) -> i64 {
    sqlx::query_scalar!(
        "SELECT lost_length FROM Stale_Dick_Shrinks WHERE uid = $1 AND created_at = current_date", uid)
        .fetch_one(db)
        .await
        .expect("couldn't read the logged loss")
}

/// The summaries queued for today, as `(internal chat id, state)`.
async fn queued_broadcasts(db: &Pool<Postgres>) -> Vec<(i64, String)> {
    sqlx::query!(
        r#"SELECT chat_id, state::text AS "state!" FROM Scheduled_Shrink_Broadcasts
            WHERE shrink_date = current_date ORDER BY chat_id"#)
        .fetch_all(db)
        .await
        .expect("couldn't read the queued summaries")
        .into_iter()
        .map(|row| (row.chat_id, row.state))
        .collect()
}

/// Every chat with something to shrink, which is what the run walks in batches.
async fn shrink_all(shrinks: &repo::Shrinks, ratio: Ratio, grace_days: DaysCount, ramp_up_days: DaysCount)
    -> repo::ShrinkBatchOutcome
{
    let candidates = shrinks.select_shrink_candidates(grace_days)
        .await.expect("couldn't select the chats due for a shrink");
    shrinks.perform_daily_shrink(&candidates, ratio, grace_days, ramp_up_days)
        .await.expect("couldn't perform the daily shrink")
}

#[tokio::test]
async fn test_perform_daily_shrink() {
    let db = fresh_db().await;
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
    users.create_or_update(user_id(victim_uid), "stale-victim")
        .await.expect("couldn't create the victim user");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;
    let victim_updated_at_before = updated_at_of(&db, victim_uid, chat_id).await;

    // A stale but zero-length dick — must be left alone (only length > 0 shrinks).
    let zero_uid = UID + 2;
    users.create_or_update(user_id(zero_uid), "zero-length")
        .await.expect("couldn't create the zero-length user");
    seed_aged_dick(&db, chat_id, zero_uid, 0, 10).await;

    // A stale, positive dick with a pending bonus attempt — shrinking it must not silently burn
    // the bonus attempt (the Dicks trigger decrements it on every update unless counteracted).
    let bonus_uid = UID + 3;
    users.create_or_update(user_id(bonus_uid), "has-bonus")
        .await.expect("couldn't create the bonus user");
    seed_aged_dick_with_bonus_attempts(&db, chat_id, bonus_uid, 100, 10, 1).await;

    let outcome = shrink_all(&shrinks, literal!(Ratio = 0.1), GRACE_DAYS, NO_RAMP).await;

    assert_eq!(outcome.victims, 2, "the two stale, positive dicks should have shrunk");
    assert_eq!(outcome.to_broadcast, 2);
    assert_eq!(outcome.inline_only, 0);
    assert_eq!(outcome.unreachable, 0);
    assert_eq!(lost_length_of(&db, victim_uid).await, 10, "loss = ceil(100 * 0.1)");
    assert_eq!(length_of(&db, victim_uid, chat_id).await, 90);

    // One summary for the chat, however many of its dicks shrank.
    assert_eq!(outcome.chats_queued, 1);
    assert_eq!(queued_broadcasts(&db).await, vec![(chat_id, "created".to_owned())]);

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

/// A chat the bot can't post to keeps shrinking — only the broadcast is skipped, and the scheduler
/// needs the flag to know that.
#[tokio::test]
async fn test_perform_daily_shrink_reports_unreachable_chats() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());
    let chats = repo::Chats::new(db.clone(), Default::default());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(100))
        .await.expect("couldn't create the dick");
    let chat_id = internal_chat_id(&db).await;

    let victim_uid = UID + 1;
    users.create_or_update(user_id(victim_uid), "stale-victim")
        .await.expect("couldn't create the victim user");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;

    chats.mark_unreachable(&TelegramChatId::new(CHAT_ID))
        .await.expect("couldn't mark the chat as unreachable");

    let outcome = shrink_all(&shrinks, literal!(Ratio = 0.1), GRACE_DAYS, NO_RAMP).await;

    assert_eq!(outcome.victims, 1);
    assert_eq!(outcome.unreachable, 1, "the chat is messageable in principle, just not right now");
    assert_eq!(outcome.to_broadcast, 0);
    assert_eq!(outcome.chats_skipped, 1);
    assert_eq!(length_of(&db, victim_uid, chat_id).await, 90, "an unreachable chat shrinks like any other");

    // Nothing is queued for it, which is the point: the worker never spends a request on a chat
    // the bot is already known to have lost.
    assert_eq!(outcome.chats_queued, 0);
    assert!(queued_broadcasts(&db).await.is_empty());
}

/// A chat known only by its `chat_instance` can't be messaged proactively at all, so it shrinks
/// without ever being queued. Its members still see the events through the `shrinks` command.
#[tokio::test]
async fn test_perform_daily_shrink_queues_nothing_for_an_inline_only_chat() {
    let db = fresh_db().await;
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    let chat_id = sqlx::query_scalar!("INSERT INTO Chats (chat_instance) VALUES ('inline-only') RETURNING id")
        .fetch_one(&db).await.expect("couldn't create the inline-only chat");

    let victim_uid = UID + 1;
    users.create_or_update(user_id(victim_uid), "stale-victim")
        .await.expect("couldn't create the victim user");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;

    let outcome = shrink_all(&shrinks, literal!(Ratio = 0.1), GRACE_DAYS, NO_RAMP).await;

    assert_eq!(outcome.victims, 1);
    assert_eq!(outcome.inline_only, 1);
    assert_eq!(outcome.chats_queued, 0);
    assert_eq!(length_of(&db, victim_uid, chat_id).await, 90);
    assert!(queued_broadcasts(&db).await.is_empty());
}

/// The unique index is what keeps a chat from being told twice about the same day, however often
/// the run is repeated — the property that makes re-running a partly failed day safe.
#[tokio::test]
async fn test_perform_daily_shrink_queues_one_summary_per_chat_and_day() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(100))
        .await.expect("couldn't create the fresh dick");
    let chat_id = internal_chat_id(&db).await;

    let victim_uid = UID + 1;
    users.create_or_update(user_id(victim_uid), "stale-victim")
        .await.expect("couldn't create the victim user");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;

    let first = shrink_all(&shrinks, literal!(Ratio = 0.1), GRACE_DAYS, NO_RAMP).await;
    assert_eq!(first.chats_queued, 1);

    // The second run aborts on Stale_Dick_Shrinks' primary key, so nothing of it lands — neither a
    // second length change nor a second summary.
    let candidates = shrinks.select_shrink_candidates(GRACE_DAYS)
        .await.expect("couldn't select the chats due for a shrink");
    let repeated = shrinks.perform_daily_shrink(&candidates, literal!(Ratio = 0.1), GRACE_DAYS, NO_RAMP).await;
    assert!(repeated.is_err(), "shrinking the same chat twice in one day must not go through");

    assert_eq!(length_of(&db, victim_uid, chat_id).await, 90);
    assert_eq!(queued_broadcasts(&db).await, vec![(chat_id, "created".to_owned())]);
}

#[tokio::test]
async fn test_perform_daily_shrink_ramps_up_the_ratio() {
    let db = fresh_db().await;
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
    users.create_or_update(user_id(just_overdue_uid), "just-overdue")
        .await.expect("couldn't create the just-overdue user");
    seed_aged_dick(&db, chat_id, just_overdue_uid, 1000, 7).await;

    // Overdue for exactly the ramp's length (grace + ramp = 11 days): the ramp has fully
    // kicked in, so the full ratio applies.
    let fully_ramped_uid = UID + 2;
    users.create_or_update(user_id(fully_ramped_uid), "fully-ramped")
        .await.expect("couldn't create the fully-ramped user");
    seed_aged_dick(&db, chat_id, fully_ramped_uid, 1000, 10).await;

    // Way overdue: the ramp is capped, so this must lose the same as the fully-ramped dick above,
    // not more.
    let way_overdue_uid = UID + 3;
    users.create_or_update(user_id(way_overdue_uid), "way-overdue")
        .await.expect("couldn't create the way-overdue user");
    seed_aged_dick(&db, chat_id, way_overdue_uid, 1000, 20).await;

    shrink_all(&shrinks, literal!(Ratio = 0.5), GRACE_DAYS, DaysCount::new(4)).await;

    assert_eq!(lost_length_of(&db, just_overdue_uid).await, 125, "1/4 of the ramp: ceil(1000 * 0.5 * 1/4)");
    assert_eq!(lost_length_of(&db, fully_ramped_uid).await, 500, "ramp fully kicked in: ceil(1000 * 0.5)");
    assert_eq!(lost_length_of(&db, way_overdue_uid).await, 500, "ramp is capped at the full ratio, not exceeded");
}

/// The loss formula floors at `GREATEST(1, ...)`, so any neglected, positive-length dick loses at
/// least 1 cm per (real, daily-cadenced) run no matter how small `ratio * length` rounds down to —
/// it can always reach exactly 0 eventually, and `LEAST(length, ...)` caps the loss at the dick's
/// current length so it can never be shrunk past 0 into negative territory. This pins down the
/// terminal step directly: a 1 cm dick with a tiny ratio still loses exactly 1 cm, landing on 0.
#[tokio::test]
async fn test_perform_daily_shrink_floor_reaches_exactly_zero() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create a dummy dick and the Chats row");
    let chat_id = internal_chat_id(&db).await;

    let victim_uid = UID + 1;
    users.create_or_update(user_id(victim_uid), "one-cm-left")
        .await.expect("couldn't create the victim");
    seed_aged_dick(&db, chat_id, victim_uid, 1, 100).await;

    // ratio * length rounds down to 0 cm on its own — GREATEST(1, ...) must still floor it at 1.
    shrink_all(&shrinks, literal!(Ratio = 0.01), GRACE_DAYS, NO_RAMP).await;

    assert_eq!(lost_length_of(&db, victim_uid).await, 1,
        "the floor must apply even though ratio * length rounds to 0");
    assert_eq!(length_of(&db, victim_uid, chat_id).await, 0);
}

/// `DaysCount` wraps a `u32`; a `grace_days` above `i32::MAX` (but still a perfectly valid `u32`,
/// e.g. from a misconfigured `DAILY_SHRINK_INACTIVITY_DAYS`) used to get bound into the query as `.value() as
/// i32`, which silently flipped negative on overflow — turning "hasn't grown in N days" into
/// "grown within N days" and shrinking *everyone*, including dicks grown moments ago, with no
/// error anywhere. `DaysCount` now embeds directly (`u32` bumped to `i64`/`bigint` on the wire,
/// narrowed back to `int4` in SQL via an explicit `::bigint::int` cast), so an out-of-range value
/// now surfaces as a genuine Postgres error instead of silently corrupting the result — fail loud,
/// not silently wrong. Nothing gets shrunk on this call, so it's a safe failure mode too.
#[tokio::test]
async fn test_perform_daily_shrink_rejects_overflowing_grace_days_instead_of_wrapping() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create a dummy dick and the Chats row");
    let chat_id = internal_chat_id(&db).await;

    let victim_uid = UID + 1;
    users.create_or_update(user_id(victim_uid), "hundred-days-old")
        .await.expect("couldn't create the victim");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 100).await;

    let absurd_grace_days = DaysCount::new(3_000_000_000); // > i32::MAX (~2.15 billion), valid u32
    let chat_ids = &[InternalChatId::new(chat_id.try_into().expect("the internal chat id must be positive"))];
    let result = shrinks.perform_daily_shrink(chat_ids, literal!(Ratio = 0.5), absurd_grace_days, NO_RAMP).await;

    assert!(result.is_err(), "an out-of-range grace_days must error, not silently wrap to negative");
    assert_eq!(length_of(&db, victim_uid, chat_id).await, 100,
        "a failed run must not have shrunk anyone (the old sign-flip bug would've shrunk everyone)");
}

#[tokio::test]
async fn test_get_shrinks_for_date_only_returns_that_day() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(100))
        .await.expect("couldn't create the fresh dick");
    let chat_id = internal_chat_id(&db).await;

    let victim_uid = UID + 1;
    users.create_or_update(user_id(victim_uid), "victim")
        .await.expect("couldn't create the victim");
    seed_aged_dick(&db, chat_id, victim_uid, 100, 10).await;

    // Today's shrink (logged with created_at = current_date).
    shrink_all(&shrinks, literal!(Ratio = 0.1), GRACE_DAYS, NO_RAMP).await;
    // An older shrink, on a different day — must not leak into today's exact-date query.
    seed_old_shrink(&db, chat_id, UID, 5, 8).await;

    let today = chrono::Utc::now().date_naive();
    let recent = shrinks.get_shrinks_for_date(&CHAT_ID_KIND, today, Offset::new(0), Limit::new(10)).await
        .expect("couldn't fetch today's shrinks");
    assert_eq!(recent.len(), 1, "only today's shrink should be returned, not the older one");
    assert_eq!(recent[0].lost_length, 10);
    assert_eq!(recent[0].length, 90, "the victim's current (post-shrink) length, joined from Dicks");
}

/// The broadcast and the inline `shrinks` command both page off `get_shrinks_for_date`, so a day
/// with more victims than fit on one page must split cleanly: same ordering, no row repeated or
/// skipped between page 0 and page 1.
#[tokio::test]
async fn test_get_shrinks_for_date_pages() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user and the Chats row");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create a dummy dick");
    let chat_id = internal_chat_id(&db).await;

    // Three shrinks logged on the same day (days_ago 1), with distinct lost_length so ordering is
    // unambiguous (get_shrinks_for_date orders by lost_length DESC).
    for (n, lost) in [(1i32, 30i64), (2, 20), (3, 10)] {
        let uid = UID + i64::from(n);
        users.create_or_update(user_id(uid), &format!("victim-{n}"))
            .await.expect("couldn't create a victim");
        seed_aged_dick(&db, chat_id, uid, 100, 10).await;
        seed_old_shrink(&db, chat_id, uid, lost, 1).await;
    }

    let date = chrono::Utc::now().date_naive() - chrono::Duration::days(1);
    let page0 = shrinks.get_shrinks_for_date(&CHAT_ID_KIND, date, Offset::new(0), Limit::new(2)).await
        .expect("couldn't fetch page 0");
    let page1 = shrinks.get_shrinks_for_date(&CHAT_ID_KIND, date, Offset::new(2), Limit::new(2)).await
        .expect("couldn't fetch page 1");

    assert_eq!(page0.len(), 2, "page 0 must be full");
    assert_eq!(page1.len(), 1, "page 1 holds the remainder");
    assert_eq!(page0[0].lost_length, 30, "the biggest loss comes first");
    assert_eq!(page0[1].lost_length, 20);
    assert_eq!(page1[0].lost_length, 10, "the smallest loss lands on the last page");

    let page0_uids: Vec<_> = page0.iter().map(|s| s.uid).collect();
    let page1_uids: Vec<_> = page1.iter().map(|s| s.uid).collect();
    assert!(page0_uids.iter().all(|u| !page1_uids.contains(u)), "the pages must be disjoint");
}

#[tokio::test]
async fn test_get_latest_shrink_date_returns_none_without_history() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user and the Chats row");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create a dummy dick");

    let date = shrinks.get_latest_shrink_date(&CHAT_ID_KIND).await
        .expect("couldn't fetch the latest shrink date");
    assert_eq!(date, None, "a chat with no logged shrinks has no latest date");
}

/// Seeds shrinks on three non-consecutive days (unbounded history — no window to fall outside of
/// anymore) and checks both the latest-date lookup and the older/newer neighbor lookups from the
/// middle date, including the `None` case at each end.
#[tokio::test]
async fn test_get_latest_and_adjacent_shrink_dates() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let shrinks = repo::Shrinks::new(db.clone());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user and the Chats row");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create a dummy dick");
    let chat_id = internal_chat_id(&db).await;

    // Non-consecutive days: 1, 3, and 8 days ago. Two shrinks on the same day (1 day ago) to also
    // confirm MAX/MIN collapse duplicates rather than erroring.
    for (n, days_ago) in [(1i32, 1i32), (2, 1), (3, 3), (4, 8)] {
        let uid = UID + i64::from(n);
        users.create_or_update(user_id(uid), &format!("victim-{n}"))
            .await.expect("couldn't create a victim");
        seed_old_shrink(&db, chat_id, uid, 10, days_ago).await;
    }

    let today = chrono::Utc::now().date_naive();
    let newest = today - chrono::Duration::days(1);
    let middle = today - chrono::Duration::days(3);
    let oldest = today - chrono::Duration::days(8);

    let latest = shrinks.get_latest_shrink_date(&CHAT_ID_KIND).await
        .expect("couldn't fetch the latest shrink date");
    assert_eq!(latest, Some(newest), "the latest date must be the most recent day with shrinks");

    let from_middle = shrinks.get_adjacent_shrink_dates(&CHAT_ID_KIND, middle).await
        .expect("couldn't fetch neighbours of the middle date");
    assert_eq!(from_middle.older, Some(oldest), "the nearest older date");
    assert_eq!(from_middle.newer, Some(newest), "the nearest newer date");

    let from_newest = shrinks.get_adjacent_shrink_dates(&CHAT_ID_KIND, newest).await
        .expect("couldn't fetch neighbours of the newest date");
    assert_eq!(from_newest.older, Some(middle));
    assert_eq!(from_newest.newer, None, "there is nothing newer than the newest date");

    let from_oldest = shrinks.get_adjacent_shrink_dates(&CHAT_ID_KIND, oldest).await
        .expect("couldn't fetch neighbours of the oldest date");
    assert_eq!(from_oldest.older, None, "there is nothing older than the oldest date");
    assert_eq!(from_oldest.newer, Some(middle));
}

#[tokio::test]
async fn test_get_player_uids() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let users = repo::Users::new(db.clone());

    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the primary user");
    dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(1))
        .await.expect("couldn't create the first dick");
    let chat_id = internal_chat_id(&db).await;

    for n in 1..=2 {
        let uid = UID + n;
        users.create_or_update(user_id(uid), &format!("player-{n}"))
            .await.expect("couldn't create a player");
        seed_aged_dick(&db, chat_id, uid, 10, 1).await;
    }

    let mut uids = dicks.get_player_uids(&CHAT_ID_KIND).await
        .expect("couldn't fetch player uids");
    uids.sort_by_key(|u| u.value());
    assert_eq!(uids, vec![
        USER_ID,
        user_id(UID + 1),
        user_id(UID + 2),
    ]);
}
