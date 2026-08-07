use sqlx::{AssertSqlSafe, Pool, Postgres};
use crate::handlers::banned_until_of;
use crate::repo;
use crate::domain::primitives::UserId;
use crate::repo::test::{fresh_db, CHAT_ID, NAME, USER_ID};

/// Every table `erase_user` must clear, as `(table, uid column)`. The guard test below fails when a
/// new one appears in the schema, because then the function needs a new DELETE too.
const TABLES_WITH_USER_ROWS: [(&str, &str); 7] = [
    ("battle_stats", "uid"),
    ("dick_of_day", "winner_uid"),
    ("dicks", "uid"),
    ("imports", "uid"),
    ("loans", "uid"),
    ("promo_code_activations", "uid"),
    ("stale_dick_shrinks", "uid"),
];

#[tokio::test]
async fn erase_user_deletes_everything_and_bans() {
    let db = fresh_db().await;
    fill_all_tables(&db).await;

    for (table, uid_field) in TABLES_WITH_USER_ROWS {
        let count = count_rows(&db, table, uid_field).await;
        assert_eq!(count, 1, "the setup didn't fill {table}");
    }

    erase(&db, 90).await;

    for (table, uid_field) in TABLES_WITH_USER_ROWS {
        let count = count_rows(&db, table, uid_field).await;
        assert_eq!(count, 0, "erase_user left rows in {table}");
    }

    let users_left = count_rows(&db, "users", "uid").await;
    assert_eq!(users_left, 1, "the Users row itself must survive an erasure");

    let user = sqlx::query!(r#"SELECT name, banned_until, created_at FROM Users WHERE uid = $1"#, USER_ID as UserId)
        .fetch_one(&db)
        .await.expect("the Users row must survive an erasure");
    assert_eq!(user.name, "", "the name must be cleared");
    let banned_until = user.banned_until.expect("the user must be banned");
    assert!(banned_until > user.created_at, "the ban must end in the future");
}

/// `erase_user` lists its tables by hand, so a new table with a user id would silently keep the
/// rows of an erased user. This test is what notices.
#[tokio::test]
async fn erase_user_covers_every_table_with_a_uid() {
    let db = fresh_db().await;

    let found = sqlx::query!(
        r#"SELECT table_name AS "table_name!", column_name AS "column_name!"
            FROM information_schema.columns
            WHERE table_schema = 'public'
              AND table_name <> 'users'
              AND column_name IN ('uid', 'winner_uid')
            ORDER BY table_name"#)
        .fetch_all(&db)
        .await.expect("couldn't read the schema");
    let found: Vec<(String, String)> = found.into_iter()
        .map(|row| (row.table_name, row.column_name))
        .collect();

    let mut expected: Vec<(String, String)> = TABLES_WITH_USER_ROWS.iter()
        .map(|(table, column)| (table.to_string(), column.to_string()))
        .collect();
    expected.sort();

    assert_eq!(found, expected, "a table with a user id was added or removed — update erase_user in the migrations and TABLES_WITH_USER_ROWS here");
}

#[tokio::test]
async fn ban_and_unban() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");

    let banned = users.get_banned()
        .await.expect("couldn't read the ban list");
    assert!(banned.is_empty(), "a fresh user must not be banned");

    ban(&db, 7).await;
    let banned = users.get_banned()
        .await.expect("couldn't read the ban list");
    assert_eq!(banned.len(), 1);
    assert_eq!(banned[0].uid, USER_ID);

    unban(&db).await;
    let banned = users.get_banned()
        .await.expect("couldn't read the ban list");
    assert!(banned.is_empty(), "the ban must be lifted");
}

#[tokio::test]
async fn get_banned_skips_expired_bans() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");

    sqlx::query!("UPDATE Users SET banned_until = current_timestamp - interval '1 day' WHERE uid = $1", USER_ID as UserId)
        .execute(&db)
        .await.expect("couldn't set an expired ban");

    let banned = users.get_banned()
        .await.expect("couldn't read the ban list");
    assert!(banned.is_empty(), "an expired ban must not be in the list");
}

#[tokio::test]
async fn a_banned_user_cannot_be_upserted() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");

    ban(&db, 90).await;

    let err = users.create_or_update(USER_ID, "a new name")
        .await.expect_err("the upsert must be refused while the user is banned");
    let date = banned_until_of(&err)
        .expect("the error must carry the ban's end date");
    assert_eq!(date.len(), 10, "the date must be formatted as DD.MM.YYYY, got {date}");

    let user = users.get_user(USER_ID)
        .await.expect("couldn't read the user")
        .expect("the user must still be there");
    assert_eq!(user.name.value(), NAME, "the name must not have changed");
}

#[tokio::test]
async fn the_admin_functions_still_work_on_a_banned_user() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");

    ban(&db, 90).await;
    ban(&db, 7).await;
    erase(&db, 90).await;
    unban(&db).await;

    let banned = users.get_banned()
        .await.expect("couldn't read the ban list");
    assert!(banned.is_empty(), "the ban must be lifted");
}

#[tokio::test]
async fn an_expired_ban_lets_the_user_be_upserted_again() {
    let db = fresh_db().await;
    let users = repo::Users::new(db.clone());
    users.create_or_update(USER_ID, NAME)
        .await.expect("couldn't create the user");

    sqlx::query!("UPDATE Users SET banned_until = current_timestamp - interval '1 day' WHERE uid = $1", USER_ID as UserId)
        .execute(&db)
        .await.expect("couldn't set an expired ban");

    users.create_or_update(USER_ID, "back again")
        .await.expect("an expired ban must not block the upsert");
}

#[tokio::test]
async fn erase_user_rejects_an_unknown_uid() {
    let db = fresh_db().await;

    let result = sqlx::query!("SELECT erase_user($1, 90)", USER_ID as UserId)
        .execute(&db)
        .await;
    assert!(result.is_err(), "erasing a user who doesn't exist must fail loudly");
}

async fn fill_all_tables(db: &Pool<Postgres>) {
    let internal_chat_id = sqlx::query!("INSERT INTO Chats (chat_id) VALUES ($1) RETURNING id", CHAT_ID)
        .fetch_one(db)
        .await.expect("couldn't create the chat")
        .id;

    sqlx::query!("INSERT INTO Users (uid, name) VALUES ($1, $2)", USER_ID as UserId, NAME)
        .execute(db).await.expect("couldn't create the user");
    sqlx::query!("INSERT INTO Dicks (uid, chat_id, length) VALUES ($1, $2, 5)", USER_ID as UserId, internal_chat_id)
        .execute(db).await.expect("couldn't create the dick");
    sqlx::query!("INSERT INTO Battle_Stats (uid, chat_id) VALUES ($1, $2)", USER_ID as UserId, internal_chat_id)
        .execute(db).await.expect("couldn't create the battle stats");
    sqlx::query!("INSERT INTO Loans (uid, chat_id, debt, payout_ratio) VALUES ($1, $2, 100, 0.1)", USER_ID as UserId, internal_chat_id)
        .execute(db).await.expect("couldn't create the loan");
    sqlx::query!("INSERT INTO Dick_of_Day (chat_id, winner_uid) VALUES ($1, $2)", internal_chat_id, USER_ID as UserId)
        .execute(db).await.expect("couldn't create the dick of the day");
    sqlx::query!("INSERT INTO Promo_Codes (code, bonus_length, capacity) VALUES ('TEST', 10, 5)")
        .execute(db).await.expect("couldn't create the promo code");
    sqlx::query!("INSERT INTO Promo_Code_Activations (uid, code, affected_chats) VALUES ($1, 'TEST', 1)", USER_ID as UserId)
        .execute(db).await.expect("couldn't create the promo code activation");
    sqlx::query!("INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length) VALUES ($1, $2, 3)", internal_chat_id, USER_ID as UserId)
        .execute(db).await.expect("couldn't create the shrink");
    sqlx::query!("INSERT INTO Imports (chat_id, uid, original_length) VALUES ($1, $2, 7)", internal_chat_id, USER_ID as UserId)
        .execute(db).await.expect("couldn't create the import");
}

/// The one query in this file that can't be a `query_scalar!`: the macro needs a string literal,
/// and the table and the column are only known while looping over [`TABLES_WITH_USER_ROWS`]. They
/// are identifiers, so they also go into the query text rather than a bind parameter, which can
/// only ever be a value. Both are `&'static str`, which keeps anything read at runtime out.
async fn count_rows(
    db: &Pool<Postgres>,
    table_name: &'static str,
    uid_field: &'static str,
) -> i64 {
    let query = format!("SELECT COUNT(*) FROM {table_name} WHERE {uid_field} = $1");
    sqlx::query_scalar(AssertSqlSafe(query))
        .bind(USER_ID)
        .fetch_one(db)
        .await.expect("couldn't count the rows")
}

async fn erase(db: &Pool<Postgres>, days: i32) {
    sqlx::query!("SELECT erase_user($1, $2)", USER_ID as UserId, days)
        .execute(db)
        .await.expect("couldn't erase the user");
}

async fn ban(db: &Pool<Postgres>, days: i32) {
    sqlx::query!("SELECT ban_user($1, $2)", USER_ID as UserId, days)
        .execute(db)
        .await.expect("couldn't ban the user");
}

async fn unban(db: &Pool<Postgres>) {
    sqlx::query!("SELECT unban_user($1)", USER_ID as UserId)
        .execute(db)
        .await.expect("couldn't unban the user");
}
