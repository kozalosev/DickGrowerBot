use sqlx::{Pool, Postgres};
use teloxide::prelude::{ChatId, UserId};
use crate::{config, repo};
use crate::repo::{BorrowResult, ChatIdKind};
use crate::repo::test::dicks::{create_dick, create_user};
use crate::repo::test::{CHAT_ID, NAME, start_postgres, UID};

#[tokio::test]
async fn test_all() {
    let (_container, db) = start_postgres().await;
    let payout_ratio = 0.1;

    create_user(&db).await;
    create_dick(&db).await; // to create a chat

    let user_id = UserId(UID as u64);
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let value: u16 = 10;

    let loans = repo::Loans::new(db.clone(), &config::AppConfig {
        loan_payout_ratio: payout_ratio,
        ..Default::default()
    });

    let no_loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans");
    assert!(no_loan.is_none());

    // the length is zero, so the user is not eligible for a loan
    let borrow_result = loans.borrow(user_id, &chat_id, value)
        .await.expect("couldn't apply for a loan with a zero length");
    assert_eq!(borrow_result, BorrowResult::NotEligible);
    let no_loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans after the rejected application");
    assert!(no_loan.is_none());

    set_length(&db, UID, CHAT_ID, -(value as i32)).await;

    let debt = 2 * value;
    let borrow_result = loans.borrow(user_id, &chat_id, debt)
        .await.expect("couldn't apply for a loan");
    assert_eq!(borrow_result, BorrowResult::Granted);

    let loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans again")
        .expect("the loan must be present");
    assert_eq!(loan.debt, debt);
    assert_eq!(loan.payout_ratio, payout_ratio);

    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let length_after_borrowing = dicks.fetch_length(user_id, &chat_id)
        .await.expect("couldn't fetch a length after borrowing");
    assert_eq!(length_after_borrowing, value as i32);

    let half_of_debt = value;
    loans.pay(user_id, &chat_id, half_of_debt)
        .await.expect("couldn't pay the loan");

    let left_to_pay = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch how much is left to pay")
        .expect("the loan, which I left to pay, must be present")
        .debt;
    assert_eq!(left_to_pay, half_of_debt);

    // the length is positive, so refinancing must be rejected as well
    // (this is the fix for the over-loaning exploit: stale confirmation buttons
    // must not grant a loan when the length is not negative anymore)
    let borrow_result = loans.borrow(user_id, &chat_id, half_of_debt)
        .await.expect("couldn't apply for a loan with a positive length");
    assert_eq!(borrow_result, BorrowResult::NotEligible);
    let untouched_debt = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans after the rejected refinancing")
        .expect("the loan must be still present")
        .debt;
    assert_eq!(untouched_debt, half_of_debt);

    set_length(&db, UID, CHAT_ID, -(half_of_debt as i32)).await;

    let borrow_result = loans.borrow(user_id, &chat_id, half_of_debt)
        .await.expect("couldn't increase the total sum of the loan");
    assert_eq!(borrow_result, BorrowResult::Granted);

    let loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans after the second borrowing")
        .expect("the loan must be present");
    assert_eq!(loan.debt, debt);
}

#[tokio::test]
async fn test_borrow_without_dick() {
    let (_container, db) = start_postgres().await;

    create_user(&db).await;
    create_dick(&db).await; // to create a chat

    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let user_id_without_dick = UserId((UID + 1) as u64);
    repo::Users::new(db.clone())
        .create_or_update(user_id_without_dick, &format!("{NAME} 2"))
        .await.expect("couldn't create a user");

    let loans = repo::Loans::new(db.clone(), &config::AppConfig {
        loan_payout_ratio: 0.1,
        ..Default::default()
    });
    let borrow_result = loans.borrow(user_id_without_dick, &chat_id, 10)
        .await.expect("couldn't apply for a loan without a dick");
    assert_eq!(borrow_result, BorrowResult::NotEligible);
}

async fn set_length(db: &Pool<Postgres>, uid: i64, chat_id: i64, length: i32) {
    // bonus_attempts = 1 bypasses the "already grown today" trigger (it decrements to 0 after)
    sqlx::query!("UPDATE Dicks SET length = $3, bonus_attempts = 1 WHERE uid = $1 AND chat_id = (SELECT id FROM Chats WHERE chat_id = $2)",
            uid, chat_id, length)
        .execute(db)
        .await
        .expect("couldn't set the dick length directly");
}
