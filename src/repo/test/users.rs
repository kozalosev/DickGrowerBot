use sqlx::{Pool, Postgres};
use teloxide::types::{ChatId, UserId};
use testcontainers::clients;
use crate::repo;
use crate::repo::ChatIdKind;
use crate::repo::test::{CHAT_ID, NAME, start_postgres, UID};

#[tokio::test]
#[ignore]
async fn create_or_update() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
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
#[ignore]
async fn get_chat_members() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
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

#[tokio::test]
#[ignore]
async fn get_random_active_member() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let users = repo::Users::new(db.clone());

    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let user = users.get_random_active_member(&chat_id)
        .await.expect("couldn't fetch None");
    assert!(user.is_none());

    create_member(&db).await;

    let user = users.get_random_active_member(&chat_id)
        .await
        .expect("couldn't fetch Some(User)")
        .expect("no active member");
    assert_eq!(user.uid, UID);
    assert_eq!(user.name, NAME);
}

fn check_user_with_name(user: &repo::User, name: &str) {
    assert_eq!(user.uid, UID);
    assert_eq!(user.name, name);
}

fn check_member_with_name(members: &[repo::User], name: &str) {
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].uid, UID);
    assert_eq!(members[0].name, name);
}

async fn create_member(db: &Pool<Postgres>) {
    let users = repo::Users::new(db.clone());
    let dicks = repo::Dicks::new(db.clone());

    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let uid = UserId(UID as u64);

    users.create_or_update(uid, NAME)
        .await.expect("couldn't create a user");
    dicks.create_or_grow(uid, &chat_id, 0)
        .await.expect("couldn't create a dick");
}
