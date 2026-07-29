use sqlx::{Pool, Postgres};
use crate::domain::primitives::{DaysCount, LengthChange, Limit, Offset, SupportedLanguage};
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

#[tokio::test]
async fn is_anchored() {
    let (_container, db) = start_postgres().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let id = TelegramChatId::new(CHAT_ID);

    // a chat the bot has never seen
    assert!(!chats.is_anchored(&id).await.expect("couldn't check an unknown chat"));

    // known by its chat_id only — the inline invocations still land in a separate row
    chats.upsert_chat(&ChatIdPartiality::Specific(id.into()))
        .await.expect("couldn't create a chat");
    assert!(!chats.is_anchored(&id).await.expect("couldn't check a half-filled chat"));

    // both ids in one row: the chat is anchored
    let full = ChatIdFull {
        id,
        instance: TelegramChatInstanceId::of("instance"),
    };
    chats.upsert_chat(&full.to_partiality(Default::default()))
        .await.expect("couldn't anchor the chat");
    assert!(chats.is_anchored(&id).await.expect("couldn't check an anchored chat"));
}

#[tokio::test]
async fn migrate_chat_id() {
    let (_container, db) = start_postgres().await;
    create_user(&db).await;

    let chats = repo::Chats::new(db.clone(), Default::default());
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let (old, new) = (TelegramChatId::new(CHAT_ID), TelegramChatId::new(-1001234567890));

    // nothing is known about the group yet — the migration is a no-op rather than an error
    chats.migrate_chat_id(&old, &new)
        .await.expect("couldn't migrate an unknown chat");
    assert!(chats.get_chat(new.into()).await.expect("couldn't fetch").is_none());

    let old_partiality = ChatIdPartiality::Specific(old.into());
    let internal_id = chats.upsert_chat(&old_partiality)
        .await.expect("couldn't create a chat");
    dicks.create_or_grow(USER_ID, &old_partiality, LengthChange::signed(5))
        .await.expect("couldn't create a dick");

    chats.migrate_chat_id(&old, &new)
        .await.expect("couldn't migrate the chat");

    // the row keeps its internal id, so the dick follows the chat to the supergroup
    let migrated = chats.get_chat(new.into())
        .await.expect("couldn't fetch the migrated chat")
        .expect("the migrated chat must exist");
    assert_eq!(migrated.internal_id, internal_id.value());
    assert!(chats.get_chat(old.into()).await.expect("couldn't fetch").is_none());

    let top = dicks.get_top(&ChatIdKind::ID(new), Offset::new(0), Limit::literal(1))
        .await.expect("couldn't fetch the top");
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].length, 5);

    // the second update of the same migration finds nothing left to do
    chats.migrate_chat_id(&old, &new)
        .await.expect("the repeated migration must be a no-op");
    assert_eq!(chats.get_chat(new.into()).await.expect("couldn't fetch")
        .expect("the chat must still exist").internal_id, internal_id.value());
}

/// A loan taken through inline mode sits on the `chat_instance` row. Anchoring the chat merges
/// that row away, so the merge has to carry the loan over instead of leaving it dangling behind
/// a `NOT NULL` foreign key.
#[tokio::test]
async fn merge_moves_dependent_rows() {
    let (_container, db) = start_postgres().await;
    create_user(&db).await;

    let chats = repo::Chats::new(db.clone(), Default::default());
    let full = ChatIdFull {
        id: TelegramChatId::new(CHAT_ID),
        instance: TelegramChatInstanceId::of("instance"),
    };

    let ids = sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, NULL), (NULL, $2) RETURNING id",
            full.id.value(), full.instance.value())
        .fetch_all(&db)
        .await.expect("couldn't create chats");
    let (id_row, instance_row) = (ids[0], ids[1]);

    sqlx::query!("INSERT INTO Loans (uid, chat_id, debt, payout_ratio) VALUES ($1, $2, 10, 0.1)", UID, instance_row)
        .execute(&db)
        .await.expect("couldn't create a loan");
    // the same user battled in both chats, so the two rows collide and have to be folded
    sqlx::query!("INSERT INTO Battle_Stats (uid, chat_id, battles_total, battles_won) VALUES ($1, $2, 3, 2), ($1, $3, 4, 1)",
            UID, instance_row, id_row)
        .execute(&db)
        .await.expect("couldn't create battle stats");

    chats.upsert_chat(&full.to_partiality(Default::default()))
        .await.expect("couldn't merge the chats");

    // everything that pointed at the merged-away row now points at the surviving one
    let loan_chat_id = sqlx::query_scalar!("SELECT chat_id FROM Loans WHERE uid = $1", UID)
        .fetch_one(&db)
        .await.expect("the loan must survive the merge");
    assert_eq!(loan_chat_id, id_row);

    let battles = sqlx::query!(r#"SELECT battles_total, battles_won FROM Battle_Stats WHERE uid = $1 AND chat_id = $2"#,
            UID, id_row)
        .fetch_one(&db)
        .await.expect("the battle stats must survive the merge");
    assert_eq!(battles.battles_total, 7);
    assert_eq!(battles.battles_won, 3);

    let leftovers = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM Battle_Stats WHERE chat_id = $1"#, instance_row)
        .fetch_one(&db)
        .await.expect("couldn't count the leftovers");
    assert_eq!(leftovers, 0);
}

#[tokio::test]
async fn merge_keeps_dicks_from_both_chats() {
    let (_container, db) = start_postgres().await;
    create_user(&db).await;
    // a second user who only ever played through inline mode
    sqlx::query!("INSERT INTO Users (uid, name) VALUES ($1, 'other')", UID + 1)
        .execute(&db)
        .await.expect("couldn't create the second user");

    let chats = repo::Chats::new(db.clone(), Default::default());
    let full = ChatIdFull {
        id: TelegramChatId::new(CHAT_ID),
        instance: TelegramChatInstanceId::of("instance"),
    };
    let ids = sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, NULL), (NULL, $2) RETURNING id",
            full.id.value(), full.instance.value())
        .fetch_all(&db)
        .await.expect("couldn't create chats");
    let (id_row, instance_row) = (ids[0], ids[1]);

    // the first user played in both chats, the second one only in the chat that gets merged away
    sqlx::query!("INSERT INTO Dicks (uid, chat_id, length, bonus_attempts) VALUES ($1, $2, 1, 2), ($1, $3, 2, 3), ($4, $3, 7, 4)",
            UID, id_row, instance_row, UID + 1)
        .execute(&db)
        .await.expect("couldn't create dicks");
    // the trigger on Dicks spends one bonus attempt per write, including the inserts above, so the
    // amounts actually stored are read back rather than assumed
    let stored = sqlx::query!("SELECT uid, chat_id, bonus_attempts FROM Dicks ORDER BY uid, chat_id")
        .fetch_all(&db)
        .await.expect("couldn't fetch the stored bonus attempts");
    let bonus_of = |u: i64, c: i64| stored.iter()
        .find(|r| r.uid == u && r.chat_id == c)
        .expect("the dick must exist")
        .bonus_attempts;
    let (kept, moved, moved_only) = (bonus_of(UID, id_row), bonus_of(UID, instance_row), bonus_of(UID + 1, instance_row));

    chats.upsert_chat(&full.to_partiality(Default::default()))
        .await.expect("couldn't merge the chats");

    let dicks = sqlx::query!(r#"SELECT uid, length AS "length: i64", bonus_attempts FROM Dicks WHERE chat_id = $1 ORDER BY uid"#, id_row)
        .fetch_all(&db)
        .await.expect("couldn't fetch the dicks");
    assert_eq!(dicks.len(), 2, "both users must keep a dick in the surviving chat");
    assert_eq!(dicks[0].length, 3, "the dicks of a user present in both chats must be summed");
    assert_eq!(dicks[1].length, 7, "a dick present only in the merged-away chat must survive");
    // the trigger decrements bonus_attempts once per write, which the merge compensates for
    assert_eq!((dicks[0].bonus_attempts, dicks[1].bonus_attempts), (kept + moved, moved_only),
        "the merge must neither lose nor invent bonus attempts");

    let leftover_dicks = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM Dicks WHERE chat_id = $1"#, instance_row)
        .fetch_one(&db)
        .await.expect("couldn't count the leftovers");
    assert_eq!(leftover_dicks, 0);
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
    let dick = dicks.get_top(&chat_id_kind, Offset::new(0), Limit::literal(1), DaysCount::new(7))
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
