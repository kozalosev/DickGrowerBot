use std::collections::HashMap;
use sqlx::{Pool, Postgres};
use crate::{config, repo};
use crate::config::Announcement;
use crate::domain::primitives::{Counter, LanguageCode, SupportedLanguage};
use crate::domain::primitives::SupportedLanguage::{EN, RU};
use crate::repo::test::{dicks, start_postgres, CHAT_ID_KIND};

#[tokio::test]
async fn test_configured() {
    let (_container, db) = start_postgres().await;
    create_chat(&db).await;

    // test creation and update
    for i in 1..=2 {
        test_configured_impl(&db, i).await;
    }
}

async fn test_configured_impl(db: &Pool<Postgres>, attempt: u8) {
    // Ensure our announcement will be shown once only:

    let announcements_config = config::AnnouncementsConfig {
        max_shows: Counter::literal(1),
        announcements: get_announcements_as_map(attempt)
    };
    let ann_repo = repo::Announcements::new(db.clone(), announcements_config);
    let [en, ru] = get_languages();

    let announcement = ann_repo.get_new(&CHAT_ID_KIND, &en)
        .await.expect("couldn't get an announcement");
    assert!(announcement.is_some());
    assert_eq!(announcement.unwrap(), format!("test {attempt}"));

    let announcement = ann_repo.get_new(&CHAT_ID_KIND, &en)
        .await.expect("couldn't get the announcement the second time");
    assert!(announcement.is_none());

    // The same test but in Russian:

    let announcement = ann_repo.get_new(&CHAT_ID_KIND, &ru)
        .await.expect("couldn't get an announcement in Russian");
    assert!(announcement.is_some());
    assert_eq!(announcement.unwrap(), format!("тест {attempt}"));

    let announcement = ann_repo.get_new(&CHAT_ID_KIND, &ru)
        .await.expect("couldn't get the announcement in Russian the second time");
    assert!(announcement.is_none());
}

#[tokio::test]
async fn test_no_announcements() {
    let (_container, db) = start_postgres().await;
    let [en, _] = get_languages();
    create_chat(&db).await;

    // Ensure we get nothing if properties are not set:

    let announcements_config = config::AnnouncementsConfig {
        max_shows: Counter::literal(1),
        announcements: Default::default()
    };
    let ann_repo = repo::Announcements::new(db.clone(), announcements_config);

    let announcement = ann_repo.get_new(&CHAT_ID_KIND, &en)
        .await.expect("couldn't get an announcement");
    assert!(announcement.is_none());

    // Ensure max_shows == 0 disables announcements completely:

    let announcements_config = config::AnnouncementsConfig {
        max_shows: Counter::literal(0),
        announcements: get_announcements_as_map(1)
    };
    let ann_repo = repo::Announcements::new(db.clone(), announcements_config);

    let announcement = ann_repo.get_new(&CHAT_ID_KIND, &en)
        .await.expect("couldn't get an announcement");
    assert!(announcement.is_none());
}

async fn create_chat(db: &Pool<Postgres>) {
    let chat_id_part = CHAT_ID_KIND.clone().into();
    dicks::create_user_and_dick_2(db, &chat_id_part, "Ann").await;
}

fn get_languages() -> [LanguageCode; 2] {
    ["en", "ru"]
        .map(ToOwned::to_owned)
        .map(LanguageCode::new)
}

fn get_announcements_as_map(n: u8) -> HashMap<SupportedLanguage, Announcement> {
    [(EN, format!("test {n}")), (RU, format!("тест {n}"))]
        .map(|(lang, ann)| (lang, Announcement::new(ann).expect("test text couldn't be empty")))
        .into_iter().collect()
}
