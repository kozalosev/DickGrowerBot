use crate::domain::primitives::LengthChange;
use crate::domain::primitives::chat::{ChatIdKind, ChatIdPartiality, TelegramChatId};
use crate::repo;
use crate::repo::test::{fresh_db, repos, CHAT_ID, USER_ID};
use crate::repo::test::dicks::create_user;

fn increment_of(value: i64) -> LengthChange {
    LengthChange::signed(value)
}

#[tokio::test]
async fn test_all() {
    let db = fresh_db().await;
    let repo::Repositories { personal_stats, dicks, .. } = repos(&db);

    let chat_id_1 = ChatIdKind::ID(TelegramChatId::new(CHAT_ID));
    let chat_id_2 = ChatIdKind::ID(TelegramChatId::new(CHAT_ID + 1));
    let uid = USER_ID;
    create_user(&db).await;

    let stats = personal_stats.get_personal_stats(uid).await
        .expect("couldn't fetch the empty stats");
    assert_eq!(stats.chats, 0);
    assert_eq!(stats.max_length, 0);
    assert_eq!(stats.total_length, 0);

    dicks.create_or_grow(uid, &ChatIdPartiality::Specific(chat_id_1.clone()), increment_of(10), &[]).await
        .expect("couldn't grow the dick in the first chat");
    dicks.create_or_grow(uid, &ChatIdPartiality::Specific(chat_id_2.clone()), increment_of(20), &[]).await
        .expect("couldn't grow the dick in the second chat");

    let stats = personal_stats.get_personal_stats(uid).await
        .expect("couldn't fetch the non-null stats");
    assert_eq!(stats.chats, 2);
    assert_eq!(stats.max_length, 20);
    assert_eq!(stats.total_length, 30);

    // Both dicks have spent their day above, so the negative lengths the stats are read from come
    // from the write that isn't subject to the once-a-day rule.
    dicks.grow_no_attempts_check(&chat_id_1, uid, increment_of(-20)).await
        .expect("couldn't shrink the dick in the first chat");
    dicks.grow_no_attempts_check(&chat_id_2, uid, increment_of(-40)).await
        .expect("couldn't shrink the dick in the second chat");
    let stats = personal_stats.get_personal_stats(uid).await
        .expect("couldn't fetch the stats with negative lengths");
    assert_eq!(stats.max_length, -10);
    assert_eq!(stats.total_length, -30);
}
