use crate::domain::primitives::Length;
use crate::repo;
use crate::repo::PromoCodeParams;
use crate::repo::test::{start_postgres, USER_ID};
use crate::repo::test::dicks::{check_dick, create_dick, create_user};

const PROMO_CODE: &str = "test10";
const PROMO_CODE_UPPERCASE: &str = "TEST10";
const PROMO_BONUS: u32 = 10;

#[tokio::test]
async fn activate() {
    let (_container, db) = start_postgres().await;

    let promo = repo::Promo::new(db.clone());
    promo.create(PromoCodeParams{
        code: PROMO_CODE.to_owned(),
        bonus_length: PROMO_BONUS,
        capacity: 1,
    }).await.expect("couldn't create a promo code");

    create_user(&db).await;
    create_dick(&db).await;
    let res = promo.activate(USER_ID, PROMO_CODE_UPPERCASE)
        .await.expect("couldn't activate the promo code");
    assert_eq!(res.chats_affected, 1);
    assert_eq!(res.bonus_length, i64::from(PROMO_BONUS));

    check_dick(&db, Length::new(PROMO_BONUS.into())).await;

    let res = promo.activate(USER_ID, PROMO_CODE).await;
    assert!(res.is_err());
}
