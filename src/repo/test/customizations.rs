use crate::customization::CustomizationId;
use crate::repo;
use crate::repo::test::{NAME, UID, USER_ID, fresh_db, user_id};

#[tokio::test]
async fn active_customization_can_be_set_replaced_and_cleared() {
    let db = fresh_db().await;
    let repo::Repositories {
        users,
        customizations,
        ..
    } = repo::test::repos(&db);
    users
        .create_or_update(USER_ID, NAME)
        .await
        .expect("couldn't create the user");

    assert_eq!(
        customizations
            .get_active(USER_ID)
            .await
            .expect("couldn't read"),
        None
    );

    customizations
        .set_active(USER_ID, CustomizationId::Crown)
        .await
        .expect("couldn't set the customization");
    assert_eq!(
        customizations
            .get_active(USER_ID)
            .await
            .expect("couldn't read"),
        Some(CustomizationId::Crown),
    );

    customizations
        .set_active(USER_ID, CustomizationId::Fire)
        .await
        .expect("couldn't replace the customization");
    assert_eq!(
        customizations
            .get_active(USER_ID)
            .await
            .expect("couldn't read"),
        Some(CustomizationId::Fire),
    );

    customizations
        .clear_active(USER_ID)
        .await
        .expect("couldn't clear the customization");
    customizations
        .clear_active(USER_ID)
        .await
        .expect("clearing an absent customization must be safe");
    assert_eq!(
        customizations
            .get_active(USER_ID)
            .await
            .expect("couldn't read"),
        None
    );
}

#[tokio::test]
async fn decorations_are_loaded_for_a_batch_and_unknown_ids_are_ignored() {
    let db = fresh_db().await;
    let repo::Repositories {
        users,
        customizations,
        ..
    } = repo::test::repos(&db);
    let unknown_uid = user_id(UID + 1);
    let plain_uid = user_id(UID + 2);
    for (uid, name) in [
        (USER_ID, NAME),
        (unknown_uid, "unknown"),
        (plain_uid, "plain"),
    ] {
        users
            .create_or_update(uid, name)
            .await
            .expect("couldn't create a user");
    }
    customizations
        .set_active(USER_ID, CustomizationId::Snake)
        .await
        .expect("couldn't set the customization");
    sqlx::query!(
        "INSERT INTO User_Customizations (uid, customization_id) VALUES ($1, 'future-id')",
        unknown_uid as crate::domain::primitives::UserId,
    )
    .execute(&db)
    .await
    .expect("couldn't insert an unknown future id");

    let decorations = customizations
        .decorations_for(&[USER_ID, unknown_uid, plain_uid])
        .await
        .expect("couldn't load the decorations");
    assert_eq!(decorations.apply(USER_ID, "known".to_owned()), "🐍 known");
    assert_eq!(
        decorations.apply(unknown_uid, "unknown".to_owned()),
        "unknown"
    );
    assert_eq!(decorations.apply(plain_uid, "plain".to_owned()), "plain");
    assert_eq!(
        customizations
            .get_active(unknown_uid)
            .await
            .expect("couldn't read"),
        None
    );
}

#[tokio::test]
async fn an_empty_batch_needs_no_rows() {
    let db = fresh_db().await;
    let customizations = repo::Customizations::new(db.clone());
    db.close().await;

    let decorations = customizations
        .decorations_for(&[])
        .await
        .expect("an empty batch must not acquire a connection");
    assert_eq!(decorations.apply(USER_ID, "plain".to_owned()), "plain");
}

#[tokio::test]
async fn deleting_the_user_cascades_to_the_customization() {
    let db = fresh_db().await;
    let repo::Repositories {
        users,
        customizations,
        ..
    } = repo::test::repos(&db);
    users
        .create_or_update(USER_ID, NAME)
        .await
        .expect("couldn't create the user");
    customizations
        .set_active(USER_ID, CustomizationId::Crown)
        .await
        .expect("couldn't set the customization");

    sqlx::query!(
        "DELETE FROM Users WHERE uid = $1",
        USER_ID as crate::domain::primitives::UserId
    )
    .execute(&db)
    .await
    .expect("couldn't delete the user");

    assert_eq!(
        customizations
            .get_active(USER_ID)
            .await
            .expect("couldn't read"),
        None
    );
}
