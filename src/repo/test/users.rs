use sqlx::{Pool, Postgres};
use teloxide::types::{ChatId, UserId};
use crate::domain::Ratio;
use crate::repo;
use crate::repo::{ChatIdKind, ChatIdPartiality};
use crate::repo::test::{CHAT_ID, NAME, start_postgres, UID};
use crate::repo::test::dicks::{create_another_user_and_dick, create_user_and_dick_2};

#[tokio::test]
async fn create_or_update() {
    let (_container, db) = start_postgres().await;
    let users = repo::Users::new(db.clone());

    let members = users.get_all().await
        .expect("couldn't fetch the empty list of members");
    assert_eq!(members.len(), 0);

    let u = users.create_or_update(UserId(UID as u64), NAME).await
        .expect("creation failed");
    check_user_with_name(&u, NAME);

    let members = users.get_all().await
        .expect("couldn't fetch the list of members after creation");
    check_member_with_name(&members, NAME);

    const NEW_NAME: &str = "foo_bar";

    let u = users.create_or_update(UserId(UID as u64), NEW_NAME).await
        .expect("creation failed");
    check_user_with_name(&u, NEW_NAME);

    let members = users.get_all().await
        .expect("couldn't fetch the list of members after update");
    check_member_with_name(&members, NEW_NAME);
}

#[tokio::test]
async fn get_chat_members() {
    let (_container, db) = start_postgres().await;
    let users = repo::Users::new(db.clone());

    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let members = users.get_chat_members(&chat_id)
        .await.expect("couldn't fetch the empty list of chat members");
    assert_eq!(members.len(), 0);

    create_member(&db).await;

    let members = users.get_chat_members(&chat_id)
        .await.expect("couldn't fetch the list of chat members");
    check_member_with_name(&members, NAME);
}

macro_rules! base_checks {
    ($db:ident, $method:ident) => {
        base_checks!($db, $method,)
    };
    ($db:ident, $method:ident, $($args:tt),*) => {
        let users = repo::Users::new($db.clone());

        let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
        let user = users.$method(&chat_id$(,$args)*)
            .await.expect("couldn't fetch None");
        assert!(user.is_none());

        create_member(&$db).await;

        let user = users.$method(&chat_id$(,$args)*)
            .await
            .expect("couldn't fetch Some(User)")
            .expect("no active member");
        assert_eq!(user.uid, UID);
        assert_eq!(user.name.value_ref(), NAME);

        // check inactive member is not found
        sqlx::query!("DROP TRIGGER IF EXISTS trg_check_and_update_dicks_timestamp ON Dicks")
            .execute(&$db)
            .await.expect("couldn't drop the trigger");
        sqlx::query!("UPDATE Dicks SET updated_at = '1997-01-01' WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1) AND uid = $2", CHAT_ID, UID)
            .execute(&$db)
            .await.expect("couldn't reset the updated_at column");

        let user = users.$method(&chat_id$(,$args)*)
            .await
            .expect("couldn't fetch Some(User)");
        assert!(user.is_none());

        sqlx::query!("UPDATE Dicks SET updated_at = now() WHERE chat_id = (SELECT id FROM Chats WHERE chat_id = $1) AND uid = $2", CHAT_ID, UID)
            .execute(&$db)
            .await.expect("couldn't rollback the updated_at column");
     };
}

#[tokio::test]
async fn get_random_active_member() {
    let (_container, db) = start_postgres().await;
    base_checks!(db, get_random_active_member);
}

#[tokio::test]
async fn get_random_active_poor_member() {
    let (_container, db) = start_postgres().await;
    let ratio = Ratio::new(0.9).unwrap();
    base_checks!(db, get_random_active_poor_member, ratio);

    // create middle-class and rich users and ensure they will never be selected as a winner
    let (users, chat_id) = prepare_for_additional_tests(&db).await;
    
    for attempt in 1..=10 {
        let user = users.get_random_active_poor_member(&chat_id.kind(), ratio)
            .await
            .unwrap_or_else(|_| panic!("couldn't fetch poor active user on attempt {attempt}"))
            .unwrap_or_else(| | panic!("nobody has been found on attempt {attempt}"));
        assert_ne!(user.uid, UID+2);
    }
}

#[ignore]   // See #60; also, the test is failed too often.
#[tokio::test]
async fn get_random_active_member_with_poor_in_priority() {
    let (_container, db) = start_postgres().await;
    base_checks!(db, get_random_active_member_with_poor_in_priority);

    // create middle-class and rich users and ensure the chance they win is the lower, the longer their dicks are
    let (users, chat_id) = prepare_for_additional_tests(&db).await;
    // test the users with negative length as well
    create_another_user_and_dick(&db, &chat_id, 4, "User-0", -10).await;
    
    let mut results = Vec::with_capacity(20);
    for attempt in 1..=100 {
        let user = users.get_random_active_member_with_poor_in_priority(&chat_id.kind())
            .await
            .unwrap_or_else(|_| panic!("couldn't fetch poor active user on attempt {attempt}"))
            .unwrap_or_else(| | panic!("nobody has been found on attempt {attempt}"));
        results.push(user.uid);
    }
    let user_0_wins = count(&results, UID+3);
    let user_1_wins = count(&results, UID);
    let user_2_wins = count(&results, UID+1);
    let user_3_wins = count(&results, UID+2);

    println!("=== DoD wins using smart mode ===");
    println!("User #0: {user_0_wins}");
    println!("User #1: {user_1_wins}");
    println!("User #2: {user_2_wins}");
    println!("User #3: {user_3_wins}");

    assert!(user_0_wins > user_1_wins);
    assert!(user_1_wins > user_2_wins);
    assert!(user_2_wins > user_3_wins);
    assert!(user_3_wins > 0);
}

async fn prepare_for_additional_tests(db: &Pool<Postgres>) -> (repo::Users, ChatIdPartiality) {
    let users = repo::Users::new(db.clone());
    let chat_id = ChatId(CHAT_ID).into();
    create_user_and_dick_2(db, &chat_id, "User-2").await;
    create_another_user_and_dick(db, &chat_id, 3, "User-3", 10).await;
    (users, chat_id)
}

fn count(v: &[i64], uid: i64) -> usize {
    v.iter()
        .filter(|u| **u == uid)
        .count()
}

fn check_user_with_name(user: &repo::User, name: &str) {
    assert_eq!(user.uid, UID);
    assert_eq!(user.name.value_ref(), name);
}

fn check_member_with_name(members: &[repo::User], name: &str) {
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].uid, UID);
    assert_eq!(members[0].name.value_ref(), name);
}

async fn create_member(db: &Pool<Postgres>) {
    let users = repo::Users::new(db.clone());
    let dicks = repo::Dicks::new(db.clone(), Default::default());

    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let uid = UserId(UID as u64);

    users.create_or_update(uid, NAME)
        .await.expect("couldn't create a user");
    dicks.create_or_grow(uid, &chat_id.into(), 0)
        .await.expect("couldn't create a dick");
}
