use chrono::Utc;
use domain_types::traits::SaturatingInto;
use domain_types::literal;
use sqlx::{Pool, Postgres};
use crate::domain::primitives::{Length, PromoBonus, PromoCapacity, PromoCode, UserId};
use crate::repo;
use crate::repo::PromoCodeParams;
use crate::repo::test::{fresh_db, USER_ID};
use crate::repo::test::dicks::{check_dick, create_dick, create_user};

const PROMO_CODE: &str = "test10";
const PROMO_CODE_UPPERCASE: &str = "TEST10";
const PROMO_BONUS: i32 = 10;

const PENALTY_PROMO_CODE: &str = "penalty30";
const PENALTY_PROMO_BONUS: i32 = -30;

#[tokio::test]
async fn activate() {
    let db = fresh_db().await;

    let promo = repo::Promo::new(db.clone());
    promo.create_promo_code(PromoCodeParams{
        code: literal!(PromoCode = PROMO_CODE),
        bonus_length: PromoBonus::new(PROMO_BONUS),
        capacity: PromoCapacity::new(1),
    }).await.expect("couldn't create a promo code");

    create_user(&db).await;
    create_dick(&db).await;
    let res = promo.activate(USER_ID, PROMO_CODE_UPPERCASE)
        .await.expect("couldn't activate the promo code");
    assert!(res.chats_affected.single());
    assert_eq!(res.bonus_length.value(), i64::from(PROMO_BONUS));

    check_dick(&db, Length::new(PROMO_BONUS.into())).await;
    check_promo_code_activations(&db).await;

    let res = promo.activate(USER_ID, PROMO_CODE).await;
    assert!(res.is_err());
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
    let res = promo.activate(USER_ID, PENALTY_PROMO_CODE)
        .await.expect("couldn't activate the promo code");
    assert!(res.chats_affected.single());
    assert_eq!(res.bonus_length.value(), i64::from(PENALTY_PROMO_BONUS));

    check_dick(&db, Length::new(PENALTY_PROMO_BONUS.into())).await;
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
