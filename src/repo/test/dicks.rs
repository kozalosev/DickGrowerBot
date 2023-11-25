use sqlx::{Pool, Postgres};
use teloxide::types::{ChatId, UserId};
use testcontainers::clients;
use crate::repo;
use crate::repo::{ChatIdFull, ChatIdKind};
use crate::repo::test::{CHAT_ID, get_chat_id_and_dicks, NAME, start_postgres, UID};

#[tokio::test]
#[ignore]
async fn test_all() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let dicks = repo::Dicks::new(db.clone());
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
    assert_eq!(growth.pos_in_top, 1);
    assert_eq!(growth.new_length, increment);
    check_top(&dicks, &chat_id, increment).await;

    let growth = dicks.set_dod_winner(&chat_id_partiality, user_id, increment as u32)
        .await
        .expect("couldn't elect a winner")
        .expect("the winner hasn't a dick");
    assert_eq!(growth.pos_in_top, 1);
    let new_length = 2 * increment;
    assert_eq!(growth.new_length, new_length);
    check_top(&dicks, &chat_id, new_length).await;
}

#[tokio::test]
#[ignore]
async fn test_top_page() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let dicks = repo::Dicks::new(db.clone());
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let chat_id_partiality = chat_id.clone().into();
    let user2_name = format!("{NAME} 2");

    // create user and dick #1
    create_user(&db).await;
    create_dick(&db).await;
    // create user and dick #2
    {
        let users = repo::Users::new(db.clone());
        let uid2 = UserId((UID + 1) as u64);
        users.create_or_update(uid2, &user2_name)
            .await.expect("couldn't create a user");
        dicks.create_or_grow(uid2, &chat_id_partiality, 1)
            .await.expect("couldn't create a dick");
    }

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
#[ignore]
async fn upsert_chat() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    create_user(&db).await;

    sqlx::query!("DROP TRIGGER IF EXISTS trg_check_and_update_dicks_timestamp ON Dicks")
        .execute(&db)
        .await.expect("couldn't drop the trigger");

    let dicks = repo::Dicks::new(db.clone());
    let chat_id_full = ChatIdFull {
        id: ChatId(CHAT_ID),
        instance: "instance".to_owned(),
    };

    old_chat_id_new_instance(&db, &dicks, chat_id_full.clone()).await;
    clear_dicks_and_chats(&db).await;

    old_instance_new_chat_id(&dicks, chat_id_full.clone()).await;
    clear_dicks_and_chats(&db).await;

    two_separate_chats(&db, &dicks, chat_id_full).await;
}

async fn clear_dicks_and_chats(db: &Pool<Postgres>) {
    sqlx::query!("DELETE FROM Dicks")
        .execute(db)
        .await.expect("couldn't delete dicks");
    sqlx::query!("DELETE FROM Chats")
        .execute(db)
        .await.expect("couldn't delete chats");
}

async fn old_chat_id_new_instance(db: &Pool<Postgres>, dicks: &repo::Dicks, full: ChatIdFull) {
    create_dick(db).await;

    let (id, inst) = (full.id, full.instance.clone());
    let chat_id = full.into();
    dicks.create_or_grow(UserId(UID as u64), &chat_id, 0)
        .await.expect("couldn't create a dick #2");

    check_chat(dicks, id, inst).await;
}

async fn old_instance_new_chat_id(dicks: &repo::Dicks, full: ChatIdFull) {
    let (id, inst) = (full.id, full.instance.clone());
    let chat_id = "instance".to_owned().into();
    dicks.create_or_grow(UserId(UID as u64), &chat_id, 0)
        .await
        .expect("couldn't create a dick #1");

    let chat_id = full.into();
    dicks.create_or_grow(UserId(UID as u64), &chat_id, 0)
        .await
        .expect("couldn't create a dick #2");

    check_chat(dicks, id, inst).await;
}

async fn two_separate_chats(db: &Pool<Postgres>, dicks: &repo::Dicks, full: ChatIdFull) {
    let (id, inst) = (full.id, full.instance.clone());
    let ids = sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, NULL), (NULL, $2) RETURNING id",
            id.0, &inst)
        .fetch_all(db)
        .await.expect("couldn't create chats");
    assert_eq!(ids.len(), 2);
    sqlx::query!("INSERT INTO Dicks (uid, chat_id, length) VALUES ($1, $2, 1), ($1, $3, 2)",
            UID, ids[0], ids[1])
        .execute(db)
        .await.expect("couldn't create dicks");

    let chat_id = full.into();
    dicks.create_or_grow(UserId(UID as u64), &chat_id, 0)
        .await
        .expect("couldn't create a dick");

    check_chat(dicks, id, inst).await;

    let chat_id_kind = chat_id.kind();
    let dick = dicks.get_top(&chat_id_kind, 0, 1)
        .await.expect("couldn't fetch the dick");
    assert_eq!(dick.len(), 1);
    assert_eq!(dick[0].length, 3);
}

async fn check_chat(dicks: &repo::Dicks, chat_id: ChatId, inst: String) {
    let chat = dicks.get_chat(chat_id.into())
        .await.expect("couldn't fetch the chat");
    assert!(chat.is_some());
    assert_eq!(chat.as_ref().unwrap().chat_id.unwrap(), chat_id.0);
    assert_eq!(chat.unwrap().chat_instance.unwrap(), inst);
}

pub async fn create_user(db: &Pool<Postgres>) {
    let users = repo::Users::new(db.clone());
    users.create_or_update(UserId(UID as u64), NAME)
        .await.expect("couldn't create a user");
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
    let d = dicks.get_top(&chat_id, 0, 1)
        .await.expect("couldn't fetch the top again");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].length, length);
    assert_eq!(d[0].owner_name, NAME);
}
