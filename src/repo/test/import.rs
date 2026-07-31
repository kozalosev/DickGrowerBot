use sqlx::{Pool, Postgres};
use teloxide::types::ChatId;
use crate::domain::objects::ExternalUser;
use crate::domain::primitives::{Length, LengthChange, UserId};
use crate::domain::primitives::chat::{ChatIdKind, TelegramChatId};
use crate::repo;
use crate::repo::test::{CHAT_ID, CHAT_ID_KIND, start_postgres, UID, USER_ID};
use crate::repo::test::dicks::{check_dick, create_dick, create_user, create_user_and_dick_2};

/// A chat the import is not asked about, used to check it stays untouched.
const OTHER_CHAT_ID: i64 = -11111;

#[tokio::test]
async fn test_all() {
    let (_container, db) = start_postgres().await;
    let import = repo::Import::new(db.clone());
    let chat_id = ChatId(CHAT_ID);

    create_user(&db).await;
    create_dick(&db).await;

    let u = import.get_imported_users(chat_id)
        .await.expect("couldn't fetch the empty list");
    assert_eq!(u.len(), 0);

    let length = Length::new(5);
    let users = vec![ExternalUser::new(USER_ID, length)];
    import.import(chat_id, &users)
        .await.expect("couldn't import users");

    let u = import.get_imported_users(chat_id)
        .await.expect("couldn't fetch the list of imported users");
    assert_eq!(u.len(), 1);
    assert_eq!(u, users);

    check_dick(&db, length).await;
}

/// Properties `import()` must uphold: an update matches only the rows it just inserted, and the
/// insert and the update behave as one atomic unit.
mod import_semantics {
    use super::*;

    /// Two users in one call must each grow by their own original_length. A dick in another chat must
    /// not move at all — that is what the join to Chats is for.
    #[tokio::test]
    async fn every_dick_grows_by_its_own_length() {
        let (_container, db) = start_postgres().await;
        let import = repo::Import::new(db.clone());
        let dicks = repo::Dicks::new(db.clone(), Default::default());
        let uid2 = UserId::literal(UID + 1);

        create_user(&db).await;
        create_dick(&db).await;
        create_user_and_dick_2(&db, &CHAT_ID_KIND.into(), "second").await;
        // The same user in another chat, so a wrong join shows up as this dick growing too.
        let other_chat = ChatIdKind::ID(TelegramChatId::new(OTHER_CHAT_ID));
        dicks.create_or_grow(USER_ID, &other_chat.into(), LengthChange::signed(3))
            .await.expect("couldn't create a dick in the other chat");

        let before1 = read_dick(&db, USER_ID, CHAT_ID).await;
        let before2 = read_dick(&db, uid2, CHAT_ID).await;
        let before_other = read_dick(&db, USER_ID, OTHER_CHAT_ID).await;

        let users = vec![
            ExternalUser::new(USER_ID, Length::new(5)),
            ExternalUser::new(uid2, Length::new(11)),
        ];
        import.import(ChatId(CHAT_ID), &users)
            .await.expect("couldn't import two users");

        let after1 = read_dick(&db, USER_ID, CHAT_ID).await;
        let after2 = read_dick(&db, uid2, CHAT_ID).await;
        assert_eq!(after1.length, before1.length + 5);
        assert_eq!(after2.length, before2.length + 11, "the second user must not get the first one's length");

        // The query adds 1 to bonus_attempts, but the BEFORE UPDATE trigger takes it back (migration
        // 8). The addition is there to get past the "already grown today" check, not to hand out an
        // attempt, so the stored value comes out unchanged.
        assert_eq!(after1.bonus_attempts, before1.bonus_attempts);
        assert_eq!(after2.bonus_attempts, before2.bonus_attempts);

        let after_other = read_dick(&db, USER_ID, OTHER_CHAT_ID).await;
        assert_eq!(after_other, before_other, "the dick in the other chat must not change");
    }

    /// Imports has a primary key on (chat_id, uid), so a second import of the same user fails. The
    /// old code leaned on the transaction to undo the INSERT; now the single statement must do it.
    #[tokio::test]
    async fn a_rejected_second_import_changes_nothing() {
        let (_container, db) = start_postgres().await;
        let import = repo::Import::new(db.clone());

        create_user(&db).await;
        create_dick(&db).await;

        let users = vec![ExternalUser::new(USER_ID, Length::new(5))];
        import.import(ChatId(CHAT_ID), &users)
            .await.expect("couldn't import the user");
        let after_first = read_dick(&db, USER_ID, CHAT_ID).await;

        let second = import.import(ChatId(CHAT_ID), &users).await;
        assert!(second.is_err(), "a second import of the same user must be rejected");

        let after_second = read_dick(&db, USER_ID, CHAT_ID).await;
        assert_eq!(after_second, after_first, "a rejected import must not grow the dick");
        let imported = import.get_imported_users(ChatId(CHAT_ID))
            .await.expect("couldn't fetch the imported users");
        assert_eq!(imported.len(), 1, "the rejected import must not add a row to Imports");
    }
}

/// One dick's length and bonus attempts, as the database holds them.
#[derive(Debug, PartialEq)]
struct StoredDick {
    length: i64,
    bonus_attempts: i32,
}

/// Reads one dick by the raw Telegram chat id, resolving the surrogate id Dicks stores.
async fn read_dick(db: &Pool<Postgres>, uid: UserId, chat_id: i64) -> StoredDick {
    let row = sqlx::query!(
            "SELECT d.length, d.bonus_attempts FROM Dicks d JOIN Chats c ON c.id = d.chat_id
                WHERE c.chat_id = $1 AND d.uid = $2",
            chat_id, uid.value())
        .fetch_one(db)
        .await
        .expect("couldn't read the dick");
    StoredDick { length: row.length, bonus_attempts: row.bonus_attempts }
}
