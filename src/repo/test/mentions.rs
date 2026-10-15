use chrono::NaiveDate;
use crate::domain::primitives::{LengthChange, Limit, Offset, Ratio};
use crate::repo;
use crate::repo::test::{fresh_db, internal_chat_id, repos, seed_aged_dick, user_id, CHAT_ID_KIND, NAME, UID, USER_ID};
use crate::domain::primitives::chat::InternalChatId;
use crate::domain::primitives::DaysCount;
use domain_types::literal;

const VICTIM_UID: i64 = UID + 1;

/// A chat with one fresh player and one victim shrunk today. Returns the chat's internal id.
async fn chat_with_a_victim(repos: &repo::Repositories, db: &sqlx::Pool<sqlx::Postgres>) -> InternalChatId {
    repos.users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the player");
    repos.dicks.create_or_grow(USER_ID, &CHAT_ID_KIND.into(), LengthChange::signed(100))
        .await.expect("couldn't create the fresh dick");
    let chat_id = internal_chat_id(db).await;

    repos.users.create_or_update(user_id(VICTIM_UID), "victim")
        .await.expect("couldn't create the victim");
    seed_aged_dick(db, chat_id, VICTIM_UID, 100, 10).await;

    let chat_id = InternalChatId::new(chat_id.try_into().expect("the internal chat id must be positive"));
    repos.shrinks.perform_daily_shrink(&[chat_id], literal!(Ratio = 0.1), DaysCount::new(7), DaysCount::new(0))
        .await.expect("couldn't shrink");
    chat_id
}

async fn victim_is_mentioned(repos: &repo::Repositories, chat_id: InternalChatId) -> bool {
    let rows = repos.shrinks.get_shrinks_for_internal_chat(chat_id, today(), Offset::new(0), Limit::new(10))
        .await.expect("couldn't fetch the shrinks");
    assert_eq!(rows.len(), 1);
    rows[0].mention
}

fn today() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

#[tokio::test]
async fn a_player_who_said_nothing_is_mentioned() {
    let db = fresh_db().await;
    let repos = repos(&db);
    let chat_id = chat_with_a_victim(&repos, &db).await;

    assert!(victim_is_mentioned(&repos, chat_id).await);
}

#[tokio::test]
async fn the_choice_for_every_chat_applies_where_nothing_else_was_chosen() {
    let db = fresh_db().await;
    let repos = repos(&db);
    let chat_id = chat_with_a_victim(&repos, &db).await;

    repos.users.set_shrink_mentions(user_id(VICTIM_UID), false)
        .await.expect("couldn't set the mentions");
    assert!(!victim_is_mentioned(&repos, chat_id).await);

    let mentions = repos.users.get_shrink_mentions(user_id(VICTIM_UID))
        .await.expect("couldn't read the mentions");
    assert!(!mentions);
}

#[tokio::test]
async fn the_choice_for_one_chat_wins_over_the_one_for_every_chat() {
    let db = fresh_db().await;
    let repos = repos(&db);
    let chat_id = chat_with_a_victim(&repos, &db).await;

    repos.users.set_shrink_mentions(user_id(VICTIM_UID), false)
        .await.expect("couldn't set the mentions");
    // Off for every chat, so the toggle turns this chat back on.
    let toggled = repos.player_settings.toggle_shrink_mentions(&CHAT_ID_KIND, user_id(VICTIM_UID))
        .await.expect("couldn't toggle the mentions");
    assert_eq!(toggled, Some(true));
    assert!(victim_is_mentioned(&repos, chat_id).await);

    let toggled = repos.player_settings.toggle_shrink_mentions(&CHAT_ID_KIND, user_id(VICTIM_UID))
        .await.expect("couldn't toggle the mentions");
    assert_eq!(toggled, Some(false));
    assert!(!victim_is_mentioned(&repos, chat_id).await);
}

#[tokio::test]
async fn the_chat_page_reports_the_same_choice() {
    let db = fresh_db().await;
    let repos = repos(&db);
    chat_with_a_victim(&repos, &db).await;

    repos.player_settings.toggle_shrink_mentions(&CHAT_ID_KIND, user_id(VICTIM_UID))
        .await.expect("couldn't toggle the mentions");
    let rows = repos.shrinks.get_shrinks_for_date(&CHAT_ID_KIND, today(), Offset::new(0), Limit::new(10))
        .await.expect("couldn't fetch the shrinks");
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].mention);
}

#[tokio::test]
async fn an_unknown_user_toggles_nothing() {
    let db = fresh_db().await;
    let repos = repos(&db);
    chat_with_a_victim(&repos, &db).await;

    let toggled = repos.player_settings.toggle_shrink_mentions(&CHAT_ID_KIND, user_id(UID + 100))
        .await.expect("couldn't toggle the mentions");
    assert_eq!(toggled, None);
}
