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

    let top = dicks.get_top(&ChatIdKind::ID(new), Offset::new(0), Limit::literal(1), DaysCount::new(7))
        .await.expect("couldn't fetch the top");
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].length, 5);

    // the second update of the same migration finds nothing left to do
    chats.migrate_chat_id(&old, &new)
        .await.expect("the repeated migration must be a no-op");
    let still_there = chats.get_chat(new.into())
        .await.expect("couldn't fetch")
        .expect("the chat must still exist");
    assert_eq!(still_there.internal_id, internal_id.value());
}

/// A loan taken through inline mode sits on the `chat_instance` row. Anchoring the chat merges
/// that row away, so the merge has to carry the loan over instead of leaving it dangling behind
/// a `NOT NULL` foreign key. The same goes for every other table referencing `Chats(id)`.
#[tokio::test]
async fn merge_moves_dependent_rows() {
    let (_container, db) = start_postgres().await;
    let chat = SplitChat::create(&db).await;

    chat.add_loan().await;
    chat.add_battle_stats().await;
    chat.add_dick_of_the_day().await;
    chat.add_shrinks().await;

    chat.merge().await;

    // everything that pointed at the merged-away row now points at the surviving one
    assert_eq!(chat.loan_chat_id().await, chat.id_row);

    assert_eq!(chat.battle_stats().await, (7, 3), "the counters of both chats must be folded");
    assert_eq!(chat.battle_stats_left().await, 0);

    // the win keeps the day it was won on instead of being restamped with today's date
    assert_eq!(chat.dick_of_the_day_days_ago().await, 1);
    assert_eq!(chat.dicks_of_the_day_left().await, 0);

    // both chats shrank the same user on the same day, so the lost lengths add up
    assert_eq!(chat.lost_length().await, 8);
}

#[tokio::test]
async fn merge_keeps_dicks_from_both_chats() {
    let (_container, db) = start_postgres().await;
    let chat = SplitChat::create(&db).await;
    // a second user who only ever played through inline mode
    sqlx::query!("INSERT INTO Users (uid, name) VALUES ($1, 'other')", UID + 1)
        .execute(&db)
        .await.expect("couldn't create the second user");

    chat.add_dicks().await;
    // the trigger on Dicks spends one bonus attempt per write, including the inserts above, so the
    // amounts actually stored are read back rather than assumed
    let kept = chat.bonus_attempts(UID, chat.id_row).await;
    let moved = chat.bonus_attempts(UID, chat.instance_row).await;
    let moved_only = chat.bonus_attempts(UID + 1, chat.instance_row).await;

    chat.merge().await;

    let dicks = chat.dicks().await;
    assert_eq!(dicks.len(), 2, "both users must keep a dick in the surviving chat");
    assert_eq!(dicks[0].0, 3, "the dicks of a user present in both chats must be summed");
    assert_eq!(dicks[1].0, 7, "a dick present only in the merged-away chat must survive");
    // the trigger decrements bonus_attempts once per write, which the merge compensates for
    assert_eq!((dicks[0].1, dicks[1].1), (kept + moved, moved_only),
        "the merge must neither lose nor invent bonus attempts");

    assert_eq!(chat.dicks_left().await, 0);
}

/// The state both merge tests start from: one row keyed by `chat_id`, another keyed by
/// `chat_instance` — exactly how a legacy group looks after being played through commands and
/// inline mode without ever having been anchored.
struct SplitChat {
    db: Pool<Postgres>,
    chats: repo::Chats,
    full: ChatIdFull,
    id_row: i64,
    instance_row: i64,
}

impl SplitChat {
    async fn create(db: &Pool<Postgres>) -> Self {
        create_user(db).await;
        let full = ChatIdFull {
            id: TelegramChatId::new(CHAT_ID),
            instance: TelegramChatInstanceId::of("instance"),
        };
        let ids = sqlx::query_scalar!("INSERT INTO Chats (chat_id, chat_instance) VALUES ($1, NULL), (NULL, $2) RETURNING id",
                full.id.value(), full.instance.value())
            .fetch_all(db)
            .await.expect("couldn't create chats");
        Self {
            db: db.clone(),
            chats: repo::Chats::new(db.clone(), Default::default()),
            full,
            id_row: ids[0],
            instance_row: ids[1],
        }
    }

    /// Anchors the chat, which is what makes the upsert find both rows and fold them into one.
    async fn merge(&self) {
        self.chats.upsert_chat(&self.full.clone().to_partiality(Default::default()))
            .await.expect("couldn't merge the chats");
    }

    async fn add_loan(&self) {
        sqlx::query!("INSERT INTO Loans (uid, chat_id, debt, payout_ratio) VALUES ($1, $2, 10, 0.1)",
                UID, self.instance_row)
            .execute(&self.db)
            .await.expect("couldn't create a loan");
    }

    /// The same user battled in both chats, so the two rows collide and have to be folded.
    async fn add_battle_stats(&self) {
        sqlx::query!("INSERT INTO Battle_Stats (uid, chat_id, battles_total, battles_won) VALUES ($1, $2, 3, 2), ($1, $3, 4, 1)",
                UID, self.instance_row, self.id_row)
            .execute(&self.db)
            .await.expect("couldn't create battle stats");
    }

    /// The insertion trigger stamps `created_at` with today's date, so a dated row can only be
    /// planted past it — which is exactly what the merge has to do to keep the history intact.
    async fn add_dick_of_the_day(&self) {
        sqlx::query!("ALTER TABLE Dick_of_Day DISABLE TRIGGER trg_check_dod_timestamp")
            .execute(&self.db)
            .await.expect("couldn't mute the trigger");
        sqlx::query!("INSERT INTO Dick_of_Day (chat_id, winner_uid, created_at) VALUES ($1, $2, current_date - 1)",
                self.instance_row, UID)
            .execute(&self.db)
            .await.expect("couldn't create a dick of the day");
        sqlx::query!("ALTER TABLE Dick_of_Day ENABLE TRIGGER trg_check_dod_timestamp")
            .execute(&self.db)
            .await.expect("couldn't restore the trigger");
    }

    async fn add_shrinks(&self) {
        sqlx::query!("INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length, created_at) VALUES ($1, $3, 5, current_date), ($2, $3, 3, current_date)",
                self.instance_row, self.id_row, UID)
            .execute(&self.db)
            .await.expect("couldn't create shrinks");
    }

    /// The first user played in both chats, the second one only in the chat that gets merged away.
    async fn add_dicks(&self) {
        sqlx::query!("INSERT INTO Dicks (uid, chat_id, length, bonus_attempts) VALUES ($1, $2, 1, 2), ($1, $3, 2, 3), ($4, $3, 7, 4)",
                UID, self.id_row, self.instance_row, UID + 1)
            .execute(&self.db)
            .await.expect("couldn't create dicks");
    }

    async fn loan_chat_id(&self) -> i64 {
        sqlx::query_scalar!("SELECT chat_id FROM Loans WHERE uid = $1", UID)
            .fetch_one(&self.db)
            .await.expect("the loan must survive the merge")
    }

    async fn battle_stats(&self) -> (i32, i32) {
        let row = sqlx::query!("SELECT battles_total, battles_won FROM Battle_Stats WHERE uid = $1 AND chat_id = $2",
                UID, self.id_row)
            .fetch_one(&self.db)
            .await.expect("the battle stats must survive the merge");
        (row.battles_total, row.battles_won)
    }

    async fn battle_stats_left(&self) -> i64 {
        sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM Battle_Stats WHERE chat_id = $1"#, self.instance_row)
            .fetch_one(&self.db)
            .await.expect("couldn't count the leftover battle stats")
    }

    async fn dick_of_the_day_days_ago(&self) -> i32 {
        sqlx::query_scalar!(r#"SELECT current_date - created_at AS "days_ago!" FROM Dick_of_Day WHERE chat_id = $1"#,
                self.id_row)
            .fetch_one(&self.db)
            .await.expect("the dick of the day must survive the merge")
    }

    async fn dicks_of_the_day_left(&self) -> i64 {
        sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM Dick_of_Day WHERE chat_id = $1"#, self.instance_row)
            .fetch_one(&self.db)
            .await.expect("couldn't count the orphaned dicks of the day")
    }

    async fn lost_length(&self) -> i64 {
        sqlx::query_scalar!(r#"SELECT lost_length AS "lost_length!" FROM Stale_Dick_Shrinks WHERE chat_id = $1"#,
                self.id_row)
            .fetch_one(&self.db)
            .await.expect("the shrinks must survive the merge")
    }

    /// The lengths and bonus attempts of the surviving chat, ordered by user.
    async fn dicks(&self) -> Vec<(i64, i32)> {
        sqlx::query!(r#"SELECT length AS "length: i64", bonus_attempts FROM Dicks WHERE chat_id = $1 ORDER BY uid"#,
                self.id_row)
            .fetch_all(&self.db)
            .await.expect("couldn't fetch the dicks")
            .iter()
            .map(|row| (row.length, row.bonus_attempts))
            .collect()
    }

    async fn bonus_attempts(&self, uid: i64, chat_row: i64) -> i32 {
        sqlx::query_scalar!("SELECT bonus_attempts FROM Dicks WHERE uid = $1 AND chat_id = $2", uid, chat_row)
            .fetch_one(&self.db)
            .await.expect("the dick must exist")
    }

    async fn dicks_left(&self) -> i64 {
        sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM Dicks WHERE chat_id = $1"#, self.instance_row)
            .fetch_one(&self.db)
            .await.expect("couldn't count the leftover dicks")
    }
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
