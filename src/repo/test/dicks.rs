use num_traits::ToPrimitive;
use sqlx::{Pool, Postgres};
use crate::config::FeatureToggles;
use crate::domain::primitives::{Bet, DaysCount, Length, LengthChange, Limit, Offset, Position};
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality};
use crate::repo;
use crate::repo::test::{user_id, CHAT_ID, CHAT_ID_KIND, get_chat_id_and_dicks, NAME, fresh_db, UID, USER_ID};
use crate::literal;

const INACTIVITY_DAYS: DaysCount = DaysCount::new(7);

fn increment_of(value: i64) -> LengthChange {
    LengthChange::signed(value)
}

async fn internal_chat_id(db: &Pool<Postgres>) -> i64 {
    sqlx::query_scalar!("SELECT id FROM Chats WHERE chat_id = $1", CHAT_ID)
        .fetch_one(db)
        .await
        .expect("couldn't resolve the internal chat id")
}

/// Inserts a `Dicks` row directly (bypassing `create_or_grow`, whose trigger always stamps
/// `updated_at = now`), so we can seed a dick that looks like it decayed to `0` a while ago —
/// mirrors `seed_aged_dick` in `repo/test/shrinks.rs`.
async fn seed_stale_zero_length_dick(db: &Pool<Postgres>, internal_chat_id: i64, uid: i64, days_ago: i32) {
    sqlx::query!(
        "INSERT INTO Dicks (uid, chat_id, length, updated_at) \
            VALUES ($1, $2, 0, current_timestamp - make_interval(days => $3))",
        uid, internal_chat_id, days_ago)
        .execute(db)
        .await
        .expect("couldn't seed a stale zero-length dick");
}

#[tokio::test]
async fn test_all() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    create_user(&db).await;

    let user_id = USER_ID;
    let chat_id = CHAT_ID_KIND;
    let chat_id_partiality = chat_id.clone().into();
    let d = dicks.get_top(&chat_id, Offset::new(0), literal!(Limit = 1), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the empty top");
    assert_eq!(d.len(), 0);

    let increment = 5;
    let growth = dicks.create_or_grow(user_id, &chat_id_partiality, increment_of(increment))
        .await.expect("couldn't grow a dick");
    assert_eq!(growth.pos_in_top, Some(Position::new(1)));
    assert_eq!(growth.new_length, increment);
    check_top(&dicks, &chat_id, increment).await;

    let growth = dicks.set_dod_winner(&chat_id_partiality, user_id, increment_of(increment))
        .await
        .expect("couldn't elect a winner")
        .expect("the winner hasn't a dick");
    assert_eq!(growth.pos_in_top, Some(Position::new(1)));
    let new_length = 2 * increment;
    assert_eq!(growth.new_length, new_length);
    check_top(&dicks, &chat_id, new_length).await;
}

#[tokio::test]
async fn test_all_with_top_pagination_disabled() {
    let db = fresh_db().await;
    let dicks = {
        let features = FeatureToggles {
            top_unlimited: false,
            ..Default::default()
        };
        repo::Dicks::new(db.clone(), features)
    };
    create_user(&db).await;

    let user_id = USER_ID;
    let chat_id = CHAT_ID_KIND;
    let chat_id_partiality = chat_id.clone().into();
    let d = dicks.get_top(&chat_id, Offset::new(0), literal!(Limit = 1), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the empty top");
    assert_eq!(d.len(), 0);

    let increment = 5;
    let growth = dicks.create_or_grow(user_id, &chat_id_partiality, increment_of(increment))
        .await.expect("couldn't grow a dick");
    assert_eq!(growth.pos_in_top, None);
    assert_eq!(growth.new_length, increment);
    check_top(&dicks, &chat_id, increment).await;

    let growth = dicks.set_dod_winner(&chat_id_partiality, user_id, increment_of(increment))
        .await
        .expect("couldn't elect a winner")
        .expect("the winner hasn't a dick");
    assert_eq!(growth.pos_in_top, None);
    let new_length = 2 * increment;
    assert_eq!(growth.new_length, new_length);
    check_top(&dicks, &chat_id, new_length).await;
}

#[tokio::test]
async fn test_top_page() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let chat_id = CHAT_ID_KIND;
    let chat_id_partiality = chat_id.clone().into();
    let user2_name = format!("{NAME} 2");

    // create user and dick #1
    create_user(&db).await;
    create_dick(&db).await;
    // create user and dick #2
    create_user_and_dick_2(&db, &chat_id_partiality, &user2_name).await;

    let top_with_user2_only = dicks.get_top(&chat_id, Offset::new(0), literal!(Limit = 1), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the top");
    assert_eq!(top_with_user2_only.len(), 1);
    assert_eq!(top_with_user2_only[0].owner_name, user2_name);
    assert_eq!(top_with_user2_only[0].length, 1);

    let top_with_user1_only = dicks.get_top(&chat_id, Offset::new(1), literal!(Limit = 1), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the top");
    assert_eq!(top_with_user1_only.len(), 1);
    assert_eq!(top_with_user1_only[0].owner_name, NAME);
    assert_eq!(top_with_user1_only[0].length, 0);
}

#[tokio::test]
async fn test_hide_inactive_zero_length_from_top() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default()); // toggle is on by default in tests
    let users = repo::Users::new(db.clone());
    let chat_id = CHAT_ID_KIND;
    let chat_id_partiality = chat_id.clone().into();

    // An active player with a positive length — always visible.
    create_user(&db).await;
    dicks.create_or_grow(USER_ID, &chat_id_partiality, increment_of(5))
        .await.expect("couldn't grow the active dick");
    let internal_chat_id = internal_chat_id(&db).await;

    // A stale, zero-length dick (settled there by the shrink job) — hidden by the toggle.
    let stale_uid = UID + 1;
    users.create_or_update(user_id(stale_uid), "stale-zero")
        .await.expect("couldn't create the stale user");
    seed_stale_zero_length_dick(&db, internal_chat_id, stale_uid, 10).await;

    // A fresh, zero-length dick (just created today) — must stay visible despite length = 0.
    let fresh_uid = UID + 2;
    users.create_or_update(user_id(fresh_uid), "fresh-zero")
        .await.expect("couldn't create the fresh user");
    dicks.create_or_grow(user_id(fresh_uid), &chat_id_partiality, increment_of(0))
        .await.expect("couldn't create the fresh zero-length dick");

    let top = dicks.get_top(&chat_id, Offset::new(0), literal!(Limit = 10), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the top");

    assert_eq!(top.len(), 2, "the stale zero-length dick must be hidden");
    assert_eq!(top[0].owner_name, NAME);
    assert_eq!(top[0].position, Some(Position::new(1)));
    assert_eq!(top[1].owner_name, "fresh-zero");
    assert_eq!(top[1].position, Some(Position::new(2)), "positions must renumber without a gap");
}

#[tokio::test]
async fn test_hide_inactive_zero_length_from_top_disabled() {
    let db = fresh_db().await;
    let dicks = {
        let features = FeatureToggles {
            hide_inactive_zero_length_from_top: false,
            ..Default::default()
        };
        repo::Dicks::new(db.clone(), features)
    };
    let users = repo::Users::new(db.clone());
    let chat_id = CHAT_ID_KIND;
    let chat_id_partiality = chat_id.clone().into();

    create_user(&db).await;
    dicks.create_or_grow(USER_ID, &chat_id_partiality, increment_of(5))
        .await.expect("couldn't grow the active dick");
    let internal_chat_id = internal_chat_id(&db).await;

    let stale_uid = UID + 1;
    users.create_or_update(user_id(stale_uid), "stale-zero")
        .await.expect("couldn't create the stale user");
    seed_stale_zero_length_dick(&db, internal_chat_id, stale_uid, 10).await;

    let top = dicks.get_top(&chat_id, Offset::new(0), literal!(Limit = 10), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the top");

    assert_eq!(top.len(), 2, "the stale zero-length dick must remain visible when the toggle is off");
}

#[tokio::test]
async fn test_pvp() {
    let db = fresh_db().await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let chat_id = CHAT_ID_KIND;
    let chat_id_part: &ChatIdPartiality = &chat_id.clone().into();
    let uid = USER_ID;
    {
        let enough = dicks.check_dick(&chat_id_part.kind(), uid, literal!(Bet = 1))
            .await.expect("couldn't check the dick #1");
        assert!(!enough);
    }
    {
        create_user(&db).await;
        dicks.create_or_grow(uid, chat_id_part, increment_of(1))
            .await
            .expect("couldn't create a dick");

        let enough = dicks.check_dick(&chat_id_part.kind(), uid, literal!(Bet = 1))
            .await.expect("couldn't check the dick #2");
        assert!(enough);
    }
    {
        let enough = dicks.check_dick(&chat_id_part.kind(), uid, literal!(Bet = 2))
            .await.expect("couldn't check the dick #3");
        assert!(!enough);
    }
    {
        create_user_and_dick_2(&db, chat_id_part, Default::default()).await;
        let uid2 = user_id(UID + 1);
        let (gr1, gr2) = dicks.move_length(chat_id_part, uid, uid2, literal!(Bet = 1))
            .await.expect("couldn't move the length");

        assert_eq!(gr1.new_length, 0);
        assert_eq!(gr2.new_length, 2);
        assert_eq!(gr2.pos_in_top, Some(Position::new(1)));
        assert_eq!(gr1.pos_in_top, Some(Position::new(2)));
    }
}

pub async fn create_user(db: &Pool<Postgres>) {
    let users = repo::Users::new(db.clone());
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create a user");
}

pub async fn create_user_and_dick_2(db: &Pool<Postgres>, chat_id: &ChatIdPartiality, name: &str) {
    create_another_user_and_dick(db, chat_id, 2, name, 1).await;
}

pub async fn create_another_user_and_dick(
    db: &Pool<Postgres>,
    chat_id: &ChatIdPartiality,
    n: u8,
    name: &str,
    increment: i64,
) {
    assert!(n > 1);
    let n = n.to_i64().expect("couldn't convert n to i64");

    let users = repo::Users::new(db.clone());
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let uid2 = user_id(UID + n - 1);
    users.create_or_update(uid2, name)
        .await.unwrap_or_else(|_| panic!("couldn't create a user #{n}"));
    dicks.create_or_grow(uid2, chat_id, increment_of(increment))
        .await.unwrap_or_else(|_| panic!("couldn't create a dick #{n}"));
}

pub async fn create_dick(db: &Pool<Postgres>) {
    let (chat_id, dicks) = get_chat_id_and_dicks(db);
    dicks.create_or_grow(USER_ID, &chat_id.into(), increment_of(0))
        .await
        .expect("couldn't create a dick");
}

pub async fn check_dick(db: &Pool<Postgres>, length: Length) {
    let (chat_id, dicks) = get_chat_id_and_dicks(db);
    let top = dicks.get_top(&chat_id, Offset::new(0), literal!(Limit = 2), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the top");
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].length, length);
    assert_eq!(top[0].owner_name, NAME);
}

async fn check_top(dicks: &repo::Dicks, chat_id: &ChatIdKind, length: i64) {
    let d = dicks.get_top(chat_id, Offset::new(0), literal!(Limit = 1), INACTIVITY_DAYS)
        .await.expect("couldn't fetch the top again");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].length, length);
    assert_eq!(d[0].owner_uid, USER_ID);
    assert_eq!(d[0].owner_name, NAME);
}
