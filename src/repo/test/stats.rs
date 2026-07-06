use crate::domain::primitives::{LengthChange, SignedLengthChange};
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality, TelegramChatId};
use crate::repo;
use crate::repo::test::{CHAT_ID, start_postgres, USER_ID};
use crate::repo::test::dicks::create_user;

fn increment_of(value: i64) -> LengthChange {
    LengthChange::Signed(SignedLengthChange::new(value))
}

#[tokio::test]
async fn test_all() {
    let (_container, db) = start_postgres().await;
    let personal_stats = repo::PersonalStatsRepo::new(db.clone());
    let dicks = repo::Dicks::new(db.clone(), Default::default());

    let chat_id_1 = ChatIdKind::ID(TelegramChatId::new(CHAT_ID));
    let chat_id_2 = ChatIdKind::ID(TelegramChatId::new(CHAT_ID + 1));
    let uid = USER_ID;
    create_user(&db).await;

    let stats = personal_stats.get(uid).await
        .expect("couldn't fetch the empty stats");
    assert_eq!(stats.chats, 0);
    assert_eq!(stats.max_length, 0);
    assert_eq!(stats.total_length, 0);

    dicks.create_or_grow(uid, &ChatIdPartiality::Specific(chat_id_1.clone()), increment_of(10)).await
        .expect("couldn't grow the dick in the first chat");
    dicks.create_or_grow(uid, &ChatIdPartiality::Specific(chat_id_2.clone()), increment_of(20)).await
        .expect("couldn't grow the dick in the second chat");

    let stats = personal_stats.get(uid).await
        .expect("couldn't fetch the non-null stats");
    assert_eq!(stats.chats, 2);
    assert_eq!(stats.max_length, 20);
    assert_eq!(stats.total_length, 30);

    sqlx::query!("DROP TRIGGER IF EXISTS trg_check_and_update_dicks_timestamp ON Dicks")
        .execute(&db)
        .await.expect("couldn't drop the trigger");

    dicks.create_or_grow(uid, &ChatIdPartiality::Specific(chat_id_1), increment_of(-20)).await
        .expect("couldn't shrink the dick in the first chat");
    dicks.create_or_grow(uid, &ChatIdPartiality::Specific(chat_id_2), increment_of(-40)).await
        .expect("couldn't shrink the dick in the second chat");
    let stats = personal_stats.get(uid).await
        .expect("couldn't fetch the stats with negative lengths");
    assert_eq!(stats.max_length, -10);
    assert_eq!(stats.total_length, -30);
}
