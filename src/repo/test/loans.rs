use teloxide::prelude::{ChatId, UserId};
use crate::{config, repo};
use crate::repo::ChatIdKind;
use crate::repo::test::dicks::{create_dick, create_user};
use crate::repo::test::{CHAT_ID, start_postgres, UID};

#[tokio::test]
async fn test_all() {
    let (_container, db) = start_postgres().await;
    let payout_ratio = 0.1;

    create_user(&db).await;
    create_dick(&db).await; // to create a chat

    let user_id = UserId(UID as u64);
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    let value: u32 = 10;

    let loans = repo::Loans::new(db.clone(), &config::AppConfig {
        loan_payout_ratio: payout_ratio,
        ..Default::default()
    });
    let no_loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans");
    assert!(no_loan.is_none());
    
    loans.borrow(user_id, &chat_id, value)
        .await.expect("couldn't apply for a loan");

    let loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans again")
        .expect("the loan must be present");
    assert_eq!(loan.debt, value);
    assert_eq!(loan.payout_ratio, payout_ratio);
    
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let length_after_borrowing = dicks.fetch_length(user_id, &chat_id)
        .await.expect("couldn't fetch a length after borrowing");
    assert_eq!(length_after_borrowing, value as i32);
    
    let half_of_debt = value / 2;
    loans.pay(user_id, &chat_id, half_of_debt)
        .await.expect("couldn't pay the loan");

    let left_to_pay = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch how much is left to pay")
        .expect("the loan, which I left to pay, must be present")
        .debt;
    assert_eq!(left_to_pay, half_of_debt);

    loans.borrow(user_id, &chat_id, half_of_debt)
        .await.expect("couldn't increase the total som of the loan");

    let loan = loans.get_active_loan(user_id, &chat_id)
        .await.expect("couldn't fetch active loans after the second borrowing")
        .expect("the loan must be present");
    assert_eq!(loan.debt, value);
}
