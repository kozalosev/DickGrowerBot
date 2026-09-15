use chrono::Utc;
use domain_types::traits::SaturatingInto;
use domain_types::literal;
use sqlx::{Pool, Postgres};
use crate::domain::primitives::{Length, PromoBonus, PromoCapacity, PromoCode, UserId};
use crate::domain::primitives::chat::ChatIdPartiality;
use crate::repo;
use crate::repo::{ActivationError, PromoCodeParams};
use crate::repo::test::{fresh_db, user_id, CHAT_ID_KIND, UID, USER_ID};
use crate::repo::test::dicks::{check_dick, create_dick, create_user, create_user_and_dick_2};

const PROMO_CODE: &str = "test10";
const PROMO_CODE_UPPERCASE: &str = "TEST10";
const PROMO_BONUS: i32 = 10;

const PENALTY_PROMO_CODE: &str = "penalty30";
const PENALTY_PROMO_BONUS: i32 = -30;

/// The column's `promo_code_format` and `promo_code_validator` have to agree on the shortest code
/// there is. The bot writes through the type, so a code the type takes and the column turns down
/// would fail as a constraint violation rather than as a refusal anyone can act on.
#[tokio::test]
async fn the_column_takes_the_shortest_code_the_type_takes() {
    let db = fresh_db().await;
    let promo = repo::Promo::new(db);

    promo.create_promo_code(PromoCodeParams {
        code: literal!(PromoCode = "abc"),
        bonus_length: PromoBonus::new(PROMO_BONUS),
        capacity: PromoCapacity::new(1),
    }).await.expect("three characters is the minimum of both");
}

#[tokio::test]
async fn activate() {
    let db = fresh_db().await;

    let promo = repo::Promo::new(db.clone());
    promo.create_promo_code(PromoCodeParams{
        code: literal!(PromoCode = PROMO_CODE),
        bonus_length: PromoBonus::new(PROMO_BONUS),
        // Two, not one: a re-activation by the same user has to be refused as AlreadyActivated
        // rather than as Exhausted, and a capacity of one would make the two indistinguishable.
        capacity: PromoCapacity::new(2),
    }).await.expect("couldn't create a promo code");

    create_user(&db).await;
    create_dick(&db).await;
    let res = promo.activate(USER_ID, &literal!(PromoCode = PROMO_CODE_UPPERCASE))
        .await.expect("couldn't activate the promo code");
    assert!(res.chats_affected.single());
    assert_eq!(res.bonus_length.value(), i64::from(PROMO_BONUS));

    check_dick(&db, Length::new(PROMO_BONUS.into())).await;
    check_promo_code_activations(&db).await;

    let res = promo.activate(USER_ID, &literal!(PromoCode = PROMO_CODE)).await;
    assert!(matches!(res, Err(ActivationError::AlreadyActivated)));
}

#[tokio::test]
async fn activate_with_negative_bonus() {
    let db = fresh_db().await;

    let promo = repo::Promo::new(db.clone());
    promo.create_promo_code(PromoCodeParams{
        code: literal!(PromoCode = PENALTY_PROMO_CODE),
        bonus_length: PromoBonus::new(PENALTY_PROMO_BONUS),
        capacity: PromoCapacity::new(1),
    }).await.expect("couldn't create a promo code");

    create_user(&db).await;
    create_dick(&db).await;
    let res = promo.activate(USER_ID, &literal!(PromoCode = PENALTY_PROMO_CODE))
        .await.expect("couldn't activate the promo code");
    assert!(res.chats_affected.single());
    assert_eq!(res.bonus_length.value(), i64::from(PENALTY_PROMO_BONUS));

    check_dick(&db, Length::new(PENALTY_PROMO_BONUS.into())).await;
}

/// A code that exists but ran out of capacity has to be told apart from one that never existed
#[tokio::test]
async fn an_exhausted_code_is_reported_as_such() {
    let db = fresh_db().await;

    let promo = repo::Promo::new(db.clone());
    promo.create_promo_code(PromoCodeParams{
        code: literal!(PromoCode = PROMO_CODE),
        bonus_length: PromoBonus::new(PROMO_BONUS),
        capacity: PromoCapacity::new(1),
    }).await.expect("couldn't create a promo code");

    create_user(&db).await;
    create_dick(&db).await;
    promo.activate(USER_ID, &literal!(PromoCode = PROMO_CODE))
        .await.expect("couldn't activate the promo code");

    let chat_id = ChatIdPartiality::from(CHAT_ID_KIND);
    create_user_and_dick_2(&db, &chat_id, "another").await;
    let uid2 = user_id(UID + 1);
    let res = promo.activate(uid2, &literal!(PromoCode = PROMO_CODE)).await;
    assert!(matches!(res, Err(ActivationError::Exhausted)));

    // The refused activation spent nothing: capacity stayed at zero rather than going negative,
    // and the second user got no row of their own.
    check_capacity(&db, 0).await;
    let activation = sqlx::query!("SELECT 1 as one FROM Promo_Code_Activations WHERE uid = $1 AND code = $2",
            uid2 as UserId, PROMO_CODE)
        .fetch_optional(&db)
        .await.expect("couldn't check for a stray activation");
    assert!(activation.is_none());
}

/// A code outside its window is refused by the side of the window it is on: one that hasn't
/// started yet must not be reported as expired. Neither refusal spends any capacity.
#[tokio::test]
async fn a_code_outside_its_window_is_reported_by_its_side() {
    let db = fresh_db().await;

    let promo = repo::Promo::new(db.clone());
    promo.create_promo_code(PromoCodeParams{
        code: literal!(PromoCode = PROMO_CODE),
        bonus_length: PromoBonus::new(PROMO_BONUS),
        capacity: PromoCapacity::new(1),
    }).await.expect("couldn't create a promo code");

    create_user(&db).await;
    create_dick(&db).await;

    sqlx::query!("UPDATE Promo_Codes SET since = current_date + 1 WHERE code = $1", PROMO_CODE)
        .execute(&db)
        .await.expect("couldn't move the window into the future");
    let res = promo.activate(USER_ID, &literal!(PromoCode = PROMO_CODE)).await;
    assert!(matches!(res, Err(ActivationError::NotStarted)));
    check_capacity(&db, 1).await;

    sqlx::query!("UPDATE Promo_Codes SET since = current_date - 2, until = current_date - 1 WHERE code = $1",
            PROMO_CODE)
        .execute(&db)
        .await.expect("couldn't move the window into the past");
    let res = promo.activate(USER_ID, &literal!(PromoCode = PROMO_CODE)).await;
    assert!(matches!(res, Err(ActivationError::Expired)));
    check_capacity(&db, 1).await;

    sqlx::query!("UPDATE Promo_Codes SET until = current_date WHERE code = $1", PROMO_CODE)
        .execute(&db)
        .await.expect("couldn't end the window today");
    promo.activate(USER_ID, &literal!(PromoCode = PROMO_CODE))
        .await.expect("a code is still valid on the last day of its window");
}

async fn check_capacity(db: &Pool<Postgres>, expected: i32) {
    let row = sqlx::query!("SELECT capacity FROM Promo_Codes WHERE code = $1", PROMO_CODE)
        .fetch_one(db)
        .await.expect("couldn't fetch the promo code's capacity");
    assert_eq!(row.capacity, expected);
}

async fn check_promo_code_activations(db: &Pool<Postgres>) {
    // activated_at is nullable in the schema; the insert always sets it, so force non-null.
    let row = sqlx::query!(
            r#"SELECT uid, code, affected_chats, activated_at as "activated_at!"
                FROM Promo_Code_Activations WHERE uid = $1 AND code = $2"#,
            USER_ID as UserId, PROMO_CODE)
        .fetch_one(db)
        .await
        .expect("couldn't fetch the promo code activation");
    let uid: i64 = USER_ID.saturating_into();
    assert_eq!(row.uid, uid);
    assert_eq!(row.code, PROMO_CODE);
    assert_eq!(row.affected_chats, 1);
    assert_eq!(row.activated_at.date_naive(), Utc::now().date_naive());
}
