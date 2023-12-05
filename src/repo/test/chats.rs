use sqlx::{Pool, Postgres};
use teloxide::types::{ChatId, UserId};
use testcontainers::clients;
use crate::repo;
use crate::repo::{ChatIdFull, ChatIdPartiality};
use crate::repo::test::{CHAT_ID, start_postgres, UID};
use crate::repo::test::dicks::create_user;

#[tokio::test]
#[ignore]
async fn upsert_chat() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    create_user(&db).await;

    sqlx::query!("DROP TRIGGER IF EXISTS trg_check_and_update_dicks_timestamp ON Dicks")
        .execute(&db)
        .await.expect("couldn't drop the trigger");

    let chats = repo::Chats::new(db.clone(), Default::default());
    let chat_id_full = ChatIdFull {
        id: ChatId(CHAT_ID),
        instance: "instance".to_owned(),
    };

    old_chat_id_new_instance(&chats, chat_id_full.clone()).await;
    clear_dicks_and_chats(&db).await;

    old_instance_new_chat_id(&chats, chat_id_full.clone()).await;
    clear_dicks_and_chats(&db).await;

    two_separate_chats(&db, &chats, chat_id_full).await;
}

async fn clear_dicks_and_chats(db: &Pool<Postgres>) {
    sqlx::query!("DELETE FROM Dicks")
        .execute(db)
        .await.expect("couldn't delete dicks");
    sqlx::query!("DELETE FROM Chats")
        .execute(db)
        .await.expect("couldn't delete chats");
}

async fn old_chat_id_new_instance(chats: &repo::Chats, full: ChatIdFull) {
    let (id, inst) = (full.id, full.instance.clone());
    chats.upsert_chat(&ChatIdPartiality::Specific(id.into()))
        .await.expect("couldn't create a chat");

    chats.upsert_chat(&full.into())
        .await.expect("couldn't update the chat");

    check_chat(chats, id, inst).await;
}

async fn old_instance_new_chat_id(chats: &repo::Chats, full: ChatIdFull) {
    let (id, inst) = (full.id, full.instance.clone());
    chats.upsert_chat(&ChatIdPartiality::Specific(inst.clone().into()))
        .await.expect("couldn't create a chat");

    chats.upsert_chat(&full.into())
        .await.expect("couldn't update the chat");

    check_chat(chats, id, inst).await;
}

async fn two_separate_chats(db: &Pool<Postgres>, chats: &repo::Chats, full: ChatIdFull) {
    let dicks = repo::Dicks::new(db.clone(), Default::default());

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

    check_chat(chats, id, inst).await;

    let chat_id_kind = chat_id.kind();
    let dick = dicks.get_top(&chat_id_kind, 0, 1)
        .await.expect("couldn't fetch the dick");
    assert_eq!(dick.len(), 1);
    assert_eq!(dick[0].length, 3);
}

async fn check_chat(chats: &repo::Chats, chat_id: ChatId, inst: String) {
    let chat = chats.get_chat(chat_id.into())
        .await.expect("couldn't fetch the chat");
    assert!(chat.is_some());
    assert_eq!(chat.as_ref().unwrap().chat_id.unwrap(), chat_id.0);
    assert_eq!(chat.unwrap().chat_instance.unwrap(), inst);
}
