use sqlx::{Pool, Postgres};
use crate::config::CustomizationsConfig;
use crate::customization::CustomizationName;
use crate::domain::primitives::UserId;
use crate::repo;
use crate::repo::test::{NAME, UID, USER_ID, fresh_db, user_id};

#[tokio::test]
async fn active_customization_can_be_set_replaced_and_cleared() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    let customizations = customizations(&db);
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");

    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the customization");
    assert_eq!(name, None);

    customizations.set_active(USER_ID, CustomizationName::of("crown"))
        .await.expect("couldn't set the customization");
    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the customization");
    assert_eq!(name, Some(CustomizationName::of("crown")));

    customizations.set_active(USER_ID, CustomizationName::of("fire"))
        .await.expect("couldn't replace the customization");
    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the customization");
    assert_eq!(name, Some(CustomizationName::of("fire")));

    customizations.clear_active(USER_ID)
        .await.expect("couldn't clear the customization");
    customizations.clear_active(USER_ID)
        .await.expect("clearing an absent customization must be safe");
    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the customization");
    assert_eq!(name, None);
}

#[tokio::test]
async fn decorations_are_loaded_for_a_batch_and_unknown_names_are_ignored() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    let customizations = customizations(&db);
    let unknown_uid = user_id(UID + 1);
    let plain_uid = user_id(UID + 2);
    for (uid, name) in [
        (USER_ID, NAME),
        (unknown_uid, "unknown"),
        (plain_uid, "plain"),
    ] {
        users.create_or_update(uid, name)
            .await.expect("couldn't create a user");
    }
    customizations.set_active(USER_ID, CustomizationName::of("snake"))
        .await.expect("couldn't set the customization");
    sqlx::query!(
        "INSERT INTO User_Customizations (uid, customization_name) VALUES ($1, 'future-name')",
        unknown_uid as UserId,
    )
        .execute(&db)
        .await.expect("couldn't insert an unknown future name");

    let decorations = customizations.decorations_for(&[USER_ID, unknown_uid, plain_uid])
        .await.expect("couldn't load the decorations");
    let known = decorations.apply(USER_ID, "known".to_owned());
    let unknown = decorations.apply(unknown_uid, "unknown".to_owned());
    let plain = decorations.apply(plain_uid, "plain".to_owned());
    assert_eq!(known, "🐍 known");
    assert_eq!(unknown, "unknown");
    assert_eq!(plain, "plain");
    let name = customizations.get_active(unknown_uid)
        .await.expect("couldn't read the customization");
    assert_eq!(name, None);
}

#[tokio::test]
async fn an_empty_batch_needs_no_rows() {
    let db = fresh_db().await;
    let customizations = customizations(&db);
    db.close().await;

    let decorations = customizations.decorations_for(&[])
        .await.expect("an empty batch must not acquire a connection");
    let plain = decorations.apply(USER_ID, "plain".to_owned());
    assert_eq!(plain, "plain");
}

#[tokio::test]
async fn deleting_the_user_cascades_to_the_customization() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    let customizations = customizations(&db);
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");
    customizations.set_active(USER_ID, CustomizationName::of("crown"))
        .await.expect("couldn't set the customization");

    sqlx::query!("DELETE FROM Users WHERE uid = $1", USER_ID as UserId)
        .execute(&db)
        .await.expect("couldn't delete the user");

    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the customization");
    assert_eq!(name, None);
}

#[tokio::test]
async fn an_unknown_customization_is_rejected_before_acquiring_a_connection() {
    let db = fresh_db().await;
    let customizations = customizations(&db);
    db.close().await;

    let result = customizations.set_active(USER_ID, CustomizationName::of("unknown")).await;
    let error = result.expect_err("an unknown customization must not be saved");
    let unknown_customization = error.to_string().contains("unknown customization");
    assert!(unknown_customization);
}

#[tokio::test]
async fn reloading_the_catalog_reaches_clones_and_removes_unknown_decorations() {
    let db = fresh_db().await;
    repo::Users::new(db.clone()).create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");
    let customizations = customizations(&db);
    let shared = customizations.clone();
    let path = std::env::temp_dir()
        .join(format!("dgb-customizations-reload-{}.yml", std::process::id()));
    let path_str = path.to_str().expect("non-UTF-8 temp path");

    std::fs::write(&path, "party: { prefix: '🎉 ', suffix: ' 🎉' }")
        .expect("couldn't write the customizations file");
    customizations.reload(path_str);
    shared.set_active(USER_ID, CustomizationName::of("party"))
        .await.expect("the new customization must reach every clone");
    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the customization");
    assert_eq!(name, Some(CustomizationName::of("party")));
    let decorations = shared.decorations_for(&[USER_ID])
        .await.expect("couldn't load the decorations");
    let decorated = decorations.apply(USER_ID, "name".to_owned());
    assert_eq!(decorated, "🎉 name 🎉");

    std::fs::write(&path, "{}").expect("couldn't clear the customizations file");
    shared.reload(path_str);
    std::fs::remove_file(&path).expect("couldn't remove the customizations file");
    let name = customizations.get_active(USER_ID)
        .await.expect("couldn't read the removed customization");
    assert_eq!(name, None);
    let decorations = customizations.decorations_for(&[USER_ID])
        .await.expect("couldn't load the decorations");
    let plain = decorations.apply(USER_ID, "name".to_owned());
    assert_eq!(plain, "name");
}

fn customizations(db: &Pool<Postgres>) -> repo::Customizations {
    let config = CustomizationsConfig::load(concat!(env!("CARGO_MANIFEST_DIR"), "/customizations.yml"));
    repo::Customizations::new(db.clone(), config)
}
