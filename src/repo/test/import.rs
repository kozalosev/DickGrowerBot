use teloxide::types::{ChatId, UserId};
use testcontainers::clients;
use crate::repo;
use crate::repo::ExternalUser;
use crate::repo::test::{CHAT_ID, start_postgres, UID};
use crate::repo::test::dicks::{check_dick, create_dick, create_user};

#[tokio::test]
async fn test_all() {
    let docker = clients::Cli::default();
    let (_container, db) = start_postgres(&docker).await;
    let import = repo::Import::new(db.clone(), Default::default());
    let chat_id = ChatId(CHAT_ID);

    create_user(&db).await;
    create_dick(&db).await;

    let u = import.get_imported_users(chat_id)
        .await.expect("couldn't fetch the empty list");
    assert_eq!(u.len(), 0);

    let length = 5;
    let users = vec![ExternalUser::new(UserId(UID as u64), length)];
    import.import(chat_id, &users)
        .await.expect("couldn't import users");

    let u = import.get_imported_users(chat_id)
        .await.expect("couldn't fetch the list of imported users");
    assert_eq!(u.len(), 1);
    assert_eq!(u, users);

    check_dick(&db, length).await;
}
