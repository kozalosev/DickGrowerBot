use sqlx::{Pool, Postgres};
use crate::domain::enums::MessageGroup;
use crate::domain::primitives::{DaysCount, DelayMinutes, LengthChange, Limit, Offset, SupportedLanguage};
use crate::domain::primitives::chat::{InternalChatId, TelegramChatId, TelegramChatInstanceId, TopicId};
use crate::domain::primitives::chat::{ChatIdFull, ChatIdKind, ChatIdPartiality, ChatIdSource};
use crate::repo;
use crate::repo::ChatMigrationOutcome;
use crate::repo::test::{fresh_db, repos, CHAT_ID, UID, USER_ID};
use crate::repo::test::dicks::create_user;

#[tokio::test]
async fn chat_language_roundtrip() {
    let db = fresh_db().await;
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
async fn allowed_topics_roundtrip() {
    let db = fresh_db().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let partiality = ChatIdPartiality::Specific(ChatIdKind::ID(TelegramChatId::new(CHAT_ID)));
    let kind = partiality.kind();
    let general = TopicId::GENERAL;
    let games = TopicId::new(42);

    // A chat that never touched the setting works everywhere.
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert!(topics.is_unrestricted());

    // The first allowed topic is what restricts the chat.
    chats.allow_topic(&partiality, games)
        .await.expect("couldn't allow the topic");
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert!(!topics.is_unrestricted());
    assert!(topics.allows(games));
    assert!(!topics.allows(general));

    // The second one is added to the first, not put in its place.
    chats.allow_topic(&partiality, general)
        .await.expect("couldn't allow the second topic");
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert_eq!(topics.count(), 2);
    assert_eq!(topics.primary(), Some(general));

    // The set doesn't grow when a topic is allowed twice.
    chats.allow_topic(&partiality, games)
        .await.expect("couldn't allow the same topic again");
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert_eq!(topics.count(), 2);

    chats.forbid_topic(&partiality, general)
        .await.expect("couldn't forbid the topic");
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert_eq!(topics.count(), 1);
    assert!(topics.allows(games));

    // Forbidding the last one leaves the bot working everywhere, not nowhere.
    chats.forbid_topic(&partiality, games)
        .await.expect("couldn't forbid the last topic");
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert!(topics.is_unrestricted());

    // And so does dropping the restriction outright.
    chats.allow_topic(&partiality, games)
        .await.expect("couldn't allow the topic again");
    chats.allow_all_topics(&partiality)
        .await.expect("couldn't allow all the topics");
    let topics = chats.get_allowed_topics(&kind)
        .await.expect("couldn't read the topics");
    assert!(topics.is_unrestricted());
}

#[tokio::test]
async fn cleanup_settings_roundtrip() {
    let db = fresh_db().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let partiality = ChatIdPartiality::Specific(ChatIdKind::ID(TelegramChatId::new(CHAT_ID)));
    let kind = partiality.kind();

    // A chat that never touched the setting decided nothing.
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert!(settings.is_default());

    // A choice about one group leaves the other three undecided. Zero is a choice of its own:
    // the chat asked for these to be kept, which is not the same as leaving it to the bot.
    chats.set_cleanup_group(&partiality, MessageGroup::Notice, DelayMinutes::new(0))
        .await.expect("couldn't ask for the notices to be kept");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert!(!settings.is_default());
    assert_eq!(settings.get(MessageGroup::Notice), Some(DelayMinutes::new(0)));
    assert_eq!(settings.get(MessageGroup::Report), None);

    chats.set_cleanup_group(&partiality, MessageGroup::Event, DelayMinutes::new(15))
        .await.expect("couldn't set the delay of the events");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert_eq!(settings.get(MessageGroup::Notice), Some(DelayMinutes::new(0)));
    assert_eq!(settings.get(MessageGroup::Event), Some(DelayMinutes::new(15)));

    // Choosing again is an overwrite, not a second entry.
    chats.set_cleanup_group(&partiality, MessageGroup::Notice, DelayMinutes::new(5))
        .await.expect("couldn't set the delay of the notices");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert_eq!(settings.get(MessageGroup::Notice), Some(DelayMinutes::new(5)));
    assert_eq!(settings.get(MessageGroup::Event), Some(DelayMinutes::new(15)));

    // One group can be handed back to the bot without touching the others.
    chats.forget_cleanup_group(&partiality, MessageGroup::Notice)
        .await.expect("couldn't forget the delay of the notices");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert_eq!(settings.get(MessageGroup::Notice), None);
    assert_eq!(settings.get(MessageGroup::Event), Some(DelayMinutes::new(15)));

    // Handing back the last one leaves a chat that looks like it never chose, which it hasn't.
    chats.forget_cleanup_group(&partiality, MessageGroup::Event)
        .await.expect("couldn't forget the delay of the events");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert!(settings.is_default());

    // The inline flag shares the key with the delays and neither writer touches the other.
    chats.set_cleanup_inline(&partiality, true)
        .await.expect("couldn't ask for the inline messages to be shrunk");
    chats.set_cleanup_group(&partiality, MessageGroup::Event, DelayMinutes::new(15))
        .await.expect("couldn't set the delay of the events again");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert_eq!(settings.compresses_inline(), Some(true));
    assert_eq!(settings.get(MessageGroup::Event), Some(DelayMinutes::new(15)));

    chats.set_cleanup_inline(&partiality, false)
        .await.expect("couldn't ask for the inline messages to be left alone");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert_eq!(settings.compresses_inline(), Some(false));
    assert_eq!(settings.get(MessageGroup::Event), Some(DelayMinutes::new(15)));

    // And the whole thing goes back at once.
    chats.reset_cleanup(&partiality)
        .await.expect("couldn't reset the cleanup settings");
    let settings = chats.get_cleanup_settings(&kind)
        .await.expect("couldn't read the cleanup settings");
    assert!(settings.is_default());
    assert_eq!(settings.get(MessageGroup::Event), None);
    assert_eq!(settings.compresses_inline(), None);
}

/// All three settings live in the same jsonb column, so each one's writes must leave the others
/// alone.
///
/// Every write is followed by reading *all* of them back: a write that wipes its neighbour has to
/// be caught at the step that did it, not several steps later when it's no longer clear which one
/// was to blame.
#[tokio::test]
async fn allowed_topics_and_language_are_independent() {
    let db = fresh_db().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let partiality = ChatIdPartiality::Specific(ChatIdKind::ID(TelegramChatId::new(CHAT_ID)));
    let kind = partiality.kind();
    let general = TopicId::GENERAL;
    let games = TopicId::new(42);

    async fn assert_settings(
        chats: &repo::Chats,
        kind: &ChatIdKind,
        expected_lang: Option<SupportedLanguage>,
        expected_topics: &[TopicId],
        expected_cleanup: Option<DelayMinutes>,
        step: &str,
    ) {
        let lang = chats.get_chat_language(kind)
            .await.expect("couldn't read the language");
        assert_eq!(lang, expected_lang, "the language is wrong after {step}");

        let topics = chats.get_allowed_topics(kind)
            .await.expect("couldn't read the topics");
        // The count pins the set down together with the checks below: `allows` is true for every
        // topic of an unrestricted chat, so on its own it would pass on a wiped restriction.
        assert_eq!(topics.count(), expected_topics.len(), "the topic count is wrong after {step}");
        for topic in expected_topics {
            assert!(topics.allows(*topic), "the topic {topic} is missing after {step}");
        }

        let cleanup = chats.get_cleanup_settings(kind)
            .await.expect("couldn't read the cleanup settings");
        assert_eq!(cleanup.get(MessageGroup::Notice), expected_cleanup,
                   "the cleanup setting is wrong after {step}");
    }

    chats.set_chat_language(&partiality, Some(SupportedLanguage::RU))
        .await.expect("couldn't set the language");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[], None, "setting the language").await;

    chats.allow_topic(&partiality, games)
        .await.expect("couldn't allow the topic");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[games], None, "allowing the first topic").await;

    chats.set_cleanup_group(&partiality, MessageGroup::Notice, DelayMinutes::new(5))
        .await.expect("couldn't set the delay of the notices");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[games], Some(DelayMinutes::new(5)), "setting a cleanup delay").await;

    chats.allow_topic(&partiality, general)
        .await.expect("couldn't allow the second topic");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[general, games], Some(DelayMinutes::new(5)), "allowing the second topic").await;

    // The language changing under an established restriction is the direction the topics have to
    // survive; the first write above only proved the empty case.
    chats.set_chat_language(&partiality, Some(SupportedLanguage::ZH))
        .await.expect("couldn't overwrite the language");
    assert_settings(&chats, &kind, Some(SupportedLanguage::ZH), &[general, games], Some(DelayMinutes::new(5)), "overwriting the language").await;

    chats.forbid_topic(&partiality, general)
        .await.expect("couldn't forbid the topic");
    assert_settings(&chats, &kind, Some(SupportedLanguage::ZH), &[games], Some(DelayMinutes::new(5)), "forbidding a topic").await;

    // Clearing removes the key rather than nulling it, which is the write most likely to take the
    // whole `settings` object down with it.
    chats.set_chat_language(&partiality, None)
        .await.expect("couldn't clear the language");
    assert_settings(&chats, &kind, None, &[games], Some(DelayMinutes::new(5)), "clearing the language").await;

    chats.allow_all_topics(&partiality)
        .await.expect("couldn't allow all the topics");
    assert_settings(&chats, &kind, None, &[], Some(DelayMinutes::new(5)), "allowing all the topics").await;

    chats.reset_cleanup(&partiality)
        .await.expect("couldn't reset the cleanup settings");
    assert_settings(&chats, &kind, None, &[], None, "resetting the cleanup settings").await;

    // And the same clearing writes in the opposite order.
    chats.allow_topic(&partiality, games)
        .await.expect("couldn't allow the topic again");
    chats.set_chat_language(&partiality, Some(SupportedLanguage::RU))
        .await.expect("couldn't set the language again");
    chats.set_cleanup_group(&partiality, MessageGroup::Notice, DelayMinutes::new(15))
        .await.expect("couldn't set the delay of the notices again");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[games], Some(DelayMinutes::new(15)), "restoring all three settings").await;

    chats.reset_cleanup(&partiality)
        .await.expect("couldn't reset the cleanup settings again");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[games], None, "resetting the cleanup settings again").await;

    chats.allow_all_topics(&partiality)
        .await.expect("couldn't allow all the topics again");
    assert_settings(&chats, &kind, Some(SupportedLanguage::RU), &[], None, "dropping the restriction").await;
}

#[tokio::test]
async fn upsert_chat() {
    let db = fresh_db().await;
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
    let db = fresh_db().await;
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

/// A failed broadcast marks the chat; the next command in it clears the mark. Nothing else does —
/// an inline query carries no `chat_id`, so it's no proof the bot is back in the chat.
#[tokio::test]
async fn unreachable_flag_lifecycle() {
    let db = fresh_db().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let id = TelegramChatId::new(CHAT_ID);
    let instance = TelegramChatInstanceId::of("instance");
    let full = ChatIdFull { id, instance: instance.clone() };

    // Marking a chat the bot has never seen doesn't create it and isn't an error.
    chats.mark_unreachable(&id)
        .await.expect("couldn't mark an unknown chat");
    let chat = chats.get_chat(id.into())
        .await.expect("couldn't fetch an unknown chat");
    assert!(chat.is_none());

    chats.upsert_chat(&ChatIdPartiality::Specific(id.into()))
        .await.expect("couldn't create a chat");
    let chat = chats.get_chat(id.into())
        .await.expect("couldn't fetch a fresh chat")
        .expect("the chat should exist");
    assert!(!chat.is_unreachable, "a fresh chat is reachable");

    chats.mark_unreachable(&id)
        .await.expect("couldn't mark the chat");
    let chat = chats.get_chat(id.into())
        .await.expect("couldn't fetch a marked chat")
        .expect("the chat should exist");
    assert!(chat.is_unreachable);

    // An inline query knows the instance only, so it lands in a row of its own and leaves the
    // marked one as it is.
    chats.upsert_chat(&full.clone().to_partiality(ChatIdSource::InlineQuery))
        .await.expect("couldn't upsert by the instance");
    let chat = chats.get_chat(id.into())
        .await.expect("couldn't fetch after an inline query")
        .expect("the chat should exist");
    assert!(chat.is_unreachable, "an inline query is no proof the bot is back");

    // A command in the chat is. It also merges the two rows into one, which must keep the mark off.
    chats.upsert_chat(&full.to_partiality(ChatIdSource::Database))
        .await.expect("couldn't upsert by the chat_id");
    let chat = chats.get_chat(id.into())
        .await.expect("couldn't fetch after a command")
        .expect("the chat should exist");
    assert!(!chat.is_unreachable, "a command in the chat clears the mark");
    assert_eq!(chat.chat_instance, Some(instance.clone()), "the rows should have been merged");
}

#[tokio::test]
async fn migrate_chat_id() {
    let db = fresh_db().await;
    create_user(&db).await;

    let repo::Repositories { chats, dicks, .. } = repos(&db);
    let (old, new) = (TelegramChatId::new(CHAT_ID), TelegramChatId::new(-1001234567890));

    // nothing is known about the group yet — the migration is a no-op rather than an error, and
    // this is the one case where the chat's history really is beyond reach
    let outcome = chats.migrate_chat_id(&old, &new)
        .await.expect("couldn't migrate an unknown chat");
    assert_eq!(outcome, ChatMigrationOutcome::Untraceable);
    assert!(chats.get_chat(new.into()).await.expect("couldn't fetch").is_none());

    let old_partiality = ChatIdPartiality::Specific(old.into());
    let internal_id = chats.upsert_chat(&old_partiality)
        .await.expect("couldn't create a chat");
    dicks.create_or_grow(USER_ID, &old_partiality, LengthChange::signed(5))
        .await.expect("couldn't create a dick");

    // this chat was never anchored, so its inline half — if it ever had one — is left behind
    let outcome = chats.migrate_chat_id(&old, &new)
        .await.expect("couldn't migrate the chat");
    assert_eq!(outcome, ChatMigrationOutcome::MigratedUnanchored);

    // the row keeps its internal id, so the dick follows the chat to the supergroup
    let migrated = chats.get_chat(new.into())
        .await.expect("couldn't fetch the migrated chat")
        .expect("the migrated chat must exist");
    assert_eq!(migrated.internal_id, internal_id.value());
    assert!(chats.get_chat(old.into()).await.expect("couldn't fetch").is_none());

    let top = dicks.get_top(&ChatIdKind::ID(new), Offset::new(0), Limit::new(1), DaysCount::new(7))
        .await.expect("couldn't fetch the top");
    assert_eq!(top.len(), 1);
    assert_eq!(top[0].length, 5);

    // The other announcement of the same migration finds nothing under the old id, which looks
    // exactly like a chat that was never known — but the chat is right there under the new id, and
    // reporting a loss here would tell a group that migrated perfectly well that it lost its
    // history.
    let outcome = chats.migrate_chat_id(&old, &new)
        .await.expect("the repeated migration must be a no-op");
    assert_eq!(outcome, ChatMigrationOutcome::MigratedUnanchored,
        "both announcements of one migration must reach the same verdict");
    let still_there = chats.get_chat(new.into())
        .await.expect("couldn't fetch")
        .expect("the chat must still exist");
    assert_eq!(still_there.internal_id, internal_id.value());

    // the journal keeps both ids; this chat had never been anchored, so it had no instance to keep
    let journal = sqlx::query!("SELECT old_chat_id, old_chat_instance, new_chat_id FROM Chat_Migrations WHERE internal_id = $1",
            internal_id as InternalChatId)
        .fetch_one(&db)
        .await.expect("the migration must be journaled");
    assert_eq!((journal.old_chat_id, journal.new_chat_id), (old.value(), new.value()));
    assert_eq!(journal.old_chat_instance, None);
}

/// An anchored chat has nothing left behind to warn about: its two halves already share a row,
/// and that row simply moves to the new id.
#[tokio::test]
async fn migrate_anchored_chat_id() {
    let db = fresh_db().await;
    let chats = repo::Chats::new(db.clone(), Default::default());
    let (old, new) = (TelegramChatId::new(CHAT_ID), TelegramChatId::new(-1001234567890));

    let full = ChatIdFull { id: old, instance: TelegramChatInstanceId::of("instance") };
    chats.upsert_chat(&full.to_partiality(Default::default()))
        .await.expect("couldn't anchor the chat");

    let outcome = chats.migrate_chat_id(&old, &new)
        .await.expect("couldn't migrate the chat");
    assert_eq!(outcome, ChatMigrationOutcome::Migrated);

    // the instance the chat had is worth keeping: the row still carries the stale one until the
    // next callback overwrites it, and the journal is the only place the pairing survives
    let old_instance = sqlx::query_scalar!("SELECT old_chat_instance FROM Chat_Migrations WHERE new_chat_id = $1",
            new.value())
        .fetch_one(&db)
        .await.expect("the migration must be journaled");
    assert_eq!(old_instance.as_deref(), Some("instance"));
}

/// A loan taken through inline mode sits on the `chat_instance` row. Anchoring the chat merges
/// that row away, so the merge has to carry the loan over instead of leaving it dangling behind
/// a `NOT NULL` foreign key. The same goes for every other table referencing `Chats(id)`.
#[tokio::test]
async fn merge_moves_dependent_rows() {
    let db = fresh_db().await;
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
    let db = fresh_db().await;
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

/// `CASCADE` so the cleanup doesn't have to enumerate every table that references `Chats` — the
/// list keeps growing (loans, battle stats, announcements, shrinks, migrations), and truncating
/// `Chats` without them fails on the foreign keys anyway.
async fn clear_dicks_and_chats(db: &Pool<Postgres>) {
    sqlx::query!("TRUNCATE Dicks, Chats CASCADE")
        .execute(db)
        .await.expect("couldn't clear dicks and chats");
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
    let dick = dicks.get_top(&chat_id_kind, Offset::new(0), Limit::new(1), DaysCount::new(7))
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
