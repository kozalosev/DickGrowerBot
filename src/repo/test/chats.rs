use sqlx::{Pool, Postgres};
use crate::domain::primitives::{LengthChange, Limit, Offset, SupportedLanguage};
use crate::domain::primitives::chat::{TelegramChatId, TelegramChatInstanceId};
use crate::domain::primitives::chat::{ChatIdFull, ChatIdKind, ChatIdPartiality};
use crate::repo;
use crate::repo::test::{CHAT_ID, start_postgres, UID, USER_ID};
use crate::repo::test::dicks::create_user;

#[tokio::test]
async fn chat_language_roundtrip() {
    let (_container, db) = start_postgres().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let partiality = ChatIdPartiality::Specific(ChatIdKind::ID(TelegramChatId::new(CHAT_ID)));
    let kind = partiality.kind();

    // No setting yet.
    let lang = chats.get_chat_language(&kind)
        .await.expect("couldn't read the language");
    assert_eq!(lang, None);

    // Set, then overwrite.
    chats.set_chat_language(&partiality, Some(SupportedLanguage::RU))
        .await.expect("couldn't set the language");
    let lang = chats.get_chat_language(&kind)
        .await.expect("couldn't read the language");
    assert_eq!(lang, Some(SupportedLanguage::RU));

    chats.set_chat_language(&partiality, Some(SupportedLanguage::ZH))
        .await.expect("couldn't overwrite the language");
    let lang = chats.get_chat_language(&kind)
        .await.expect("couldn't read the language");
    assert_eq!(lang, Some(SupportedLanguage::ZH));

    // Reset back to per-user resolution.
    chats.set_chat_language(&partiality, None)
        .await.expect("couldn't clear the language");
    let lang = chats.get_chat_language(&kind)
        .await.expect("couldn't read the language");
    assert_eq!(lang, None);
}

#[tokio::test]
async fn upsert_chat() {
    let (_container, db) = start_postgres().await;
    create_user(&db).await;

    sqlx::query!("DROP TRIGGER IF EXISTS trg_check_and_update_dicks_timestamp ON Dicks")
        .execute(&db)
        .await.expect("couldn't drop the trigger");

    let chats = repo::Chats::new(db.clone(), Default::default());
    let chat_id_full = ChatIdFull {
        id: TelegramChatId::new(CHAT_ID),
        instance: TelegramChatInstanceId::of("instance"),
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

    chats.upsert_chat(&full.to_partiality(Default::default()))
        .await.expect("couldn't update the chat");

    check_chat(chats, id, inst).await;
}

async fn old_instance_new_chat_id(chats: &repo::Chats, full: ChatIdFull) {
    let (id, inst) = (full.id, full.instance.clone());
    chats.upsert_chat(&ChatIdPartiality::Specific(ChatIdKind::Instance(inst.clone())))
        .await.expect("couldn't create a chat");

    chats.upsert_chat(&full.to_partiality(Default::default()))
        .await.expect("couldn't update the chat");

    check_chat(chats, id, inst).await;
}

async fn two_separate_chats(db: &Pool<Postgres>, chats: &repo::Chats, full: ChatIdFull) {
    let dicks = repo::Dicks::new(db.clone(), Default::default());

    let (id, inst) = (full.id, full.instance.clone());
    let ids = sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, NULL), (NULL, $2) RETURNING id",
            id.value(), inst.value())
        .fetch_all(db)
        .await.expect("couldn't create chats");
    assert_eq!(ids.len(), 2);
    sqlx::query!("INSERT INTO Dicks (uid, chat_id, length) VALUES ($1, $2, 1), ($1, $3, 2)",
            UID, ids[0], ids[1])
        .execute(db)
        .await.expect("couldn't create dicks");

    let chat_id = full.to_partiality(Default::default());
    dicks.create_or_grow(USER_ID, &chat_id, LengthChange::signed(0))
        .await
        .expect("couldn't create a dick");

    check_chat(chats, id, inst).await;

    let chat_id_kind = chat_id.kind();
    let dick = dicks.get_top(&chat_id_kind, Offset::new(0), Limit::literal(1))
        .await.expect("couldn't fetch the dick");
    assert_eq!(dick.len(), 1);
    assert_eq!(dick[0].length, 3);
}

async fn check_chat(chats: &repo::Chats, chat_id: TelegramChatId, inst: TelegramChatInstanceId) {
    let chat = chats.get_chat(chat_id.into())
        .await.expect("couldn't fetch the chat");
    assert!(chat.is_some());
    assert_eq!(chat.as_ref().unwrap().chat_id.unwrap(), chat_id.value());
    assert_eq!(chat.unwrap().chat_instance.as_deref().unwrap(), inst.value());
}
