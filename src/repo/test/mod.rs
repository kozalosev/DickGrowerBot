mod users;
mod dicks;
mod chats;
mod import;
mod promo;
mod loans;
mod pvpstats;
mod stats;
mod shrinks;
mod announcements;
mod bans;

use std::str::FromStr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::OnceCell;
use reqwest::Url;
use domain_types::traits::SaturatingInto;
use sqlx::{Pool, Postgres};
use sqlx::postgres::PgPoolOptions;
use sqlx::AssertSqlSafe;
use crate::config::DatabaseConfig;
use crate::domain::primitives::UserId;
use crate::domain::primitives::chat::TelegramChatId;
use crate::repo;
use crate::repo::ChatIdKind;
use crate::test_containers::{self, SharedContainer};

const POSTGRES_USER: &str = "test";
const POSTGRES_PASSWORD: &str = "test_pw";
const POSTGRES_DB: &str = "test_db";
/// Migrated once per run and then copied for each test — see [`SharedPostgres::start`].
const TEMPLATE_DB: &str = "test_template";
/// Prefix of the per-test databases, so they can be told apart from the two above and dropped.
const TEST_DB_PREFIX: &str = "test_run";
const POSTGRES_PORT: u16 = 5432;

/// The raw column value, so that a test can do arithmetic on it and hand it to a query. The domain
/// value is [`USER_ID`], and [`user_id`] makes one out of a computed number.
pub const UID: i64 = 12345;
pub const CHAT_ID: i64 = -67890;
pub const NAME: &str = "test";

pub const USER_ID: UserId = UserId::new(UID as u64);
pub const CHAT_ID_KIND: ChatIdKind = ChatIdKind::ID(TelegramChatId::new(CHAT_ID));

static CONTAINER: SharedContainer = SharedContainer::new(
        "postgres", "postgres", "latest", POSTGRES_PORT,
        // Postgres prints this while setting itself up and again when it is really up.
        &["PostgreSQL init process complete; ready for start up.",
          "PostgreSQL init process complete; ready for start up."])
    .with_settle_millis(300)
    .with_env(&[("POSTGRES_USER", POSTGRES_USER),
                ("POSTGRES_PASSWORD", POSTGRES_PASSWORD),
                ("POSTGRES_DB", POSTGRES_DB)]);

static POSTGRES: OnceCell<SharedPostgres> = OnceCell::const_new();

/// A migrated database of this test's own, inside the container shared by the whole binary.
///
/// The pool is built on the caller's runtime, so it dies with the test that made it; everything
/// shared stays on [`RUNTIME`].
pub async fn fresh_db() -> Pool<Postgres> {
    let url = test_containers::spawn(
        async { POSTGRES.get_or_init(SharedPostgres::start).await.create_database().await })
        .await
        .expect("the shared Postgres task failed");
    connect_and_migrate(url).await
}

/// One Postgres for the whole binary, reused across runs, with a database per test.
struct SharedPostgres {
    port: u16,
    pool: Pool<Postgres>,
    /// Distinguishes this run's databases from those of an earlier one in the reused container.
    run: u32,
    databases_created: AtomicU32,
}

impl SharedPostgres {
    async fn start() -> Self {
        let port = CONTAINER.port().await;
        let pool = PgPoolOptions::new()
            // Every test asks for its database through this one pool, and they all start at once,
            // so a single connection would make them queue up until one times out.
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(60))
            .connect(db_url(port, POSTGRES_DB).as_str())
            .await.expect("couldn't connect to the maintenance database");

        drop_databases_of_earlier_runs(&pool).await;

        // The migrations run once, here — and are rebuilt every run, since they may have changed
        // since the reused container last saw them. Every test's database is then a copy of this
        // template, which is far cheaper than replaying every migration for every test.
        for stmt in [format!("DROP DATABASE IF EXISTS {TEMPLATE_DB} WITH (FORCE)"),
                     format!("CREATE DATABASE {TEMPLATE_DB}")] {
            sqlx::query(AssertSqlSafe(stmt.clone()))
                .execute(&pool)
                .await.unwrap_or_else(|e| panic!("couldn't run `{stmt}`: {e}"));
        }
        let template = connect_and_migrate(db_url(port, TEMPLATE_DB)).await;
        // `CREATE DATABASE ... TEMPLATE` refuses to run while anyone is connected to the template.
        template.close().await;

        Self { port, pool, run: std::process::id(), databases_created: AtomicU32::new(0) }
    }

    async fn create_database(&self) -> Url {
        let n = self.databases_created.fetch_add(1, Ordering::Relaxed);
        let name = format!("{TEST_DB_PREFIX}{}_{n}", self.run);

        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name} TEMPLATE {TEMPLATE_DB}")))
            .execute(&self.pool)
            .await.unwrap_or_else(|e| panic!("couldn't create the database {name}: {e}"));

        db_url(self.port, &name)
    }
}

/// The container survives the run, so the databases of previous runs are still in it. This assumes
/// one test binary at a time, which is how `cargo test` runs them.
async fn drop_databases_of_earlier_runs(pool: &Pool<Postgres>) {
    let leftovers: Vec<String> = sqlx::query_scalar(
        "SELECT datname FROM pg_database WHERE datname LIKE $1")
        .bind(format!("{TEST_DB_PREFIX}%"))
        .fetch_all(pool)
        .await.expect("couldn't list the leftover databases");

    for name in leftovers {
        sqlx::query(AssertSqlSafe(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")))
            .execute(pool)
            .await.unwrap_or_else(|e| panic!("couldn't drop the leftover database {name}: {e}"));
    }
}

fn db_url(port: u16, database: &str) -> Url {
    Url::from_str(&format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{port}/{database}"))
        .expect("invalid database URL")
}

async fn connect_and_migrate(url: Url) -> Pool<Postgres> {
    // A test drives its database from one task, so a couple of connections is plenty. The cap
    // matters because every test in the binary holds a pool of its own at the same time.
    let conf = DatabaseConfig { url, max_connections: 2, min_connections: 1 };
    repo::establish_database_connection(&conf)
        .await.expect("couldn't establish a database connection")
}

/// A user id a test computes instead of writing down, like `UID + 1` or an id taken from a loop.
/// Such a value is not known while the code is compiled, so `literal!` does not fit.
pub fn user_id(value: i64) -> UserId {
    UserId::new(value.saturating_into())
}

#[inline]
pub fn get_chat_id_and_dicks(db: &Pool<Postgres>) -> (ChatIdKind, repo::Dicks) {
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let chat_id = ChatIdKind::ID(TelegramChatId::new(CHAT_ID));
    (chat_id, dicks)
}
