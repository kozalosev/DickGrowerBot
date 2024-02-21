use sqlx::{Pool, Postgres};
use teloxide::types::{ChatId, UserId};
use testcontainers::clients;
use crate::config::FeatureToggles;
use crate::repo;
use crate::repo::{ChatIdKind, ChatIdPartiality};
use crate::repo::test::{CHAT_ID, get_chat_id_and_dicks, NAME, start_postgres, UID};

#[tokio::test]
async fn test_all() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    create_user(&db).await;

    let user_id = UserId(UID as u64);
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let chat_id_partiality = chat_id.clone().into();
    let d = dicks.get_top(&chat_id, 0, 1)
        .await.expect("couldn't fetch the empty top");
    assert_eq!(d.len(), 0);

    let increment = 5;
    let growth = dicks.create_or_grow(user_id, &chat_id_partiality, increment)
        .await.expect("couldn't grow a dick");
    assert_eq!(growth.pos_in_top, Some(1));
    assert_eq!(growth.new_length, increment);
    check_top(&dicks, &chat_id, increment).await;

    let growth = dicks.set_dod_winner(&chat_id_partiality, user_id, increment as u32)
        .await
        .expect("couldn't elect a winner")
        .expect("the winner hasn't a dick");
    assert_eq!(growth.pos_in_top, Some(1));
    let new_length = 2 * increment;
    assert_eq!(growth.new_length, new_length);
    check_top(&dicks, &chat_id, new_length).await;
}

#[tokio::test]
async fn test_all_with_top_pagination_disabled() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let dicks = {
        let features = FeatureToggles {
            top_unlimited: false,
            ..Default::default()
        };
        repo::Dicks::new(db.clone(), features)
    };
    create_user(&db).await;

    let user_id = UserId(UID as u64);
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let chat_id_partiality = chat_id.clone().into();
    let d = dicks.get_top(&chat_id, 0, 1)
        .await.expect("couldn't fetch the empty top");
    assert_eq!(d.len(), 0);

    let increment = 5;
    let growth = dicks.create_or_grow(user_id, &chat_id_partiality, increment)
        .await.expect("couldn't grow a dick");
    assert_eq!(growth.pos_in_top, None);
    assert_eq!(growth.new_length, increment);
    check_top(&dicks, &chat_id, increment).await;

    let growth = dicks.set_dod_winner(&chat_id_partiality, user_id, increment as u32)
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
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let chat_id_partiality = chat_id.clone().into();
    let user2_name = format!("{NAME} 2");

    // create user and dick #1
    create_user(&db).await;
    create_dick(&db).await;
    // create user and dick #2
    create_user_and_dick_2(&db, &chat_id_partiality, &user2_name).await;

    let top_with_user2_only = dicks.get_top(&chat_id, 0, 1)
        .await.expect("couldn't fetch the top");
    assert_eq!(top_with_user2_only.len(), 1);
    assert_eq!(top_with_user2_only[0].owner_name, user2_name);
    assert_eq!(top_with_user2_only[0].length, 1);

    let top_with_user1_only = dicks.get_top(&chat_id, 1, 1)
        .await.expect("couldn't fetch the top");
    assert_eq!(top_with_user1_only.len(), 1);
    assert_eq!(top_with_user1_only[0].owner_name, NAME);
    assert_eq!(top_with_user1_only[0].length, 0);
}

#[tokio::test]
async fn test_pvp() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let chat_id_part: &ChatIdPartiality = &chat_id.clone().into();
    let uid = UserId(UID as u64);
    {
        let enough = dicks.check_dick(&chat_id_part.kind(), uid, 1)
            .await.expect("couldn't check the dick #1");
        assert!(!enough);
    }
    {
        create_user(&db).await;
        dicks.create_or_grow(uid, chat_id_part, 1)
            .await
            .expect("couldn't create a dick");

        let enough = dicks.check_dick(&chat_id_part.kind(), uid, 1)
            .await.expect("couldn't check the dick #2");
        assert!(enough);
    }
    {
        let enough = dicks.check_dick(&chat_id_part.kind(), uid, 2)
            .await.expect("couldn't check the dick #3");
        assert!(!enough);
    }
    {
        create_user_and_dick_2(&db, chat_id_part, Default::default()).await;
        let uid2 = UserId((UID + 1) as u64);
        let (gr1, gr2) = dicks.move_length(chat_id_part, uid, uid2, 1)
            .await.expect("couldn't move the length");

        assert_eq!(gr1.new_length, 0);
        assert_eq!(gr2.new_length, 2);
        assert_eq!(gr2.pos_in_top, Some(1));
        assert_eq!(gr1.pos_in_top, Some(2));
    }
}

pub async fn create_user(db: &Pool<Postgres>) {
    let users = repo::Users::new(db.clone(), Default::default());
    users.create_or_update(UserId(UID as u64), NAME)
        .await.expect("couldn't create a user");
}

async fn create_user_and_dick_2(db: &Pool<Postgres>, chat_id: &ChatIdPartiality, name: &str) {
        let users = repo::Users::new(db.clone(), Default::default());
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let uid2 = UserId((UID + 1) as u64);
        users.create_or_update(uid2, name)
            .await.expect("couldn't create a user #2");
        dicks.create_or_grow(uid2, chat_id, 1)
            .await.expect("couldn't create a dick #2");
}

pub async fn create_dick(db: &Pool<Postgres>) {
    let (chat_id, dicks) = get_chat_id_and_dicks(db);
    dicks.create_or_grow(UserId(UID as u64), &chat_id.into(), 0)
        .await
        .expect("couldn't create a dick");
}

pub async fn check_dick(db: &Pool<Postgres>, length: u32) {
    let (chat_id, dicks) = get_chat_id_and_dicks(db);
    let top = dicks.get_top(&chat_id, 0, 2)
        .await.expect("couldn't fetch the top");
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].length, length as i32);
    assert_eq!(top[0].owner_name, NAME);
}

async fn check_top(dicks: &repo::Dicks, chat_id: &ChatIdKind, length: i32) {
    let d = dicks.get_top(chat_id, 0, 1)
        .await.expect("couldn't fetch the top again");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].length, length);
    assert_eq!(d[0].owner_name, NAME);
}
