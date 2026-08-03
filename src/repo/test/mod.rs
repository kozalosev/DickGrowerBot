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
use std::sync::{Arc, Weak};
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::Mutex;
use reqwest::Url;
use sqlx::{Pool, Postgres};
use sqlx::postgres::PgPoolOptions;
use sqlx::AssertSqlSafe;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use crate::config::DatabaseConfig;
use crate::domain::primitives::UserId;
use crate::domain::primitives::chat::TelegramChatId;
use crate::repo;
use crate::repo::ChatIdKind;

/// Put on every container the tests start; `task test:clean` removes the leftovers by it.
pub const TEST_CONTAINER_LABEL: &str = "dickgrowerbot.test";

const POSTGRES_USER: &str = "test";
const POSTGRES_PASSWORD: &str = "test_pw";
const POSTGRES_DB: &str = "test_db";
const POSTGRES_PORT: u16 = 5432;

pub const UID: i64 = 12345;
pub const CHAT_ID: i64 = -67890;
pub const NAME: &str = "test";

pub const USER_ID: UserId = UserId::literal(UID);
pub const CHAT_ID_KIND: ChatIdKind = ChatIdKind::ID(TelegramChatId::new(CHAT_ID));

/// One container for a whole test file, with a separate database per test.
///
/// Starting a container costs seconds; creating a database inside a running one costs
/// milliseconds. Every test still gets its own database, so nothing leaks between them, and they
/// can keep running in parallel.
pub struct SharedPostgres {
    /// Kept alive for as long as the cell is; dropping it stops the container.
    _container: ContainerAsync<GenericImage>,
    port: u16,
    databases_created: AtomicU32,
}

impl SharedPostgres {
    pub async fn start() -> Self {
        let (container, port) = start_container().await;
        Self { _container: container, port, databases_created: AtomicU32::new(0) }
    }

    /// A new, empty, fully migrated database of its own.
    pub async fn fresh_db(&self) -> Pool<Postgres> {
        let n = self.databases_created.fetch_add(1, Ordering::Relaxed);
        let name = format!("{POSTGRES_DB}_{n}");

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(db_url(self.port, POSTGRES_DB).as_str())
            .await.expect("couldn't connect to the maintenance database");
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&pool)
            .await.unwrap_or_else(|e| panic!("couldn't create the database {name}: {e}"));
        pool.close().await;

        connect_and_migrate(db_url(self.port, &name)).await
    }
}

/// Holds the container of one test file. A [`Weak`] rather than a `OnceCell`, so the container is
/// stopped once the last test using it is done — a `static` is never dropped, and a leftover
/// Postgres would keep holding a port and a few hundred megabytes until `task test:clean`.
pub struct SharedPostgresCell(Mutex<Weak<SharedPostgres>>);

impl SharedPostgresCell {
    pub const fn new() -> Self {
        Self(Mutex::const_new(Weak::new()))
    }

    /// The running container, started on the first call. Keep the handle alive while you use it.
    pub async fn get(&self) -> Arc<SharedPostgres> {
        let mut weak = self.0.lock().await;
        if let Some(running) = weak.upgrade() {
            return running
        }
        let started = Arc::new(SharedPostgres::start().await);
        *weak = Arc::downgrade(&started);
        started
    }
}

pub async fn start_postgres() -> (ContainerAsync<GenericImage>, Pool<Postgres>) {
    let (container, port) = start_container().await;
    let pool = connect_and_migrate(db_url(port, POSTGRES_DB)).await;
    (container, pool)
}

fn db_url(port: u16, database: &str) -> Url {
    Url::from_str(&format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{port}/{database}"))
        .expect("invalid database URL")
}

async fn connect_and_migrate(url: Url) -> Pool<Postgres> {
    let conf = DatabaseConfig { url, max_connections: 10 };
    repo::establish_database_connection(&conf)
        .await.expect("couldn't establish a database connection")
}

async fn start_container() -> (ContainerAsync<GenericImage>, u16) {
    let postgres_container = GenericImage::new("postgres", "latest")
        .with_exposed_port(POSTGRES_PORT.tcp())
        .with_wait_for(WaitFor::message_on_stdout("PostgreSQL init process complete; ready for start up."))
        .with_wait_for(WaitFor::message_on_stdout("PostgreSQL init process complete; ready for start up."))
        .with_wait_for(WaitFor::millis(300))
        .with_env_var("POSTGRES_USER", POSTGRES_USER)
        .with_env_var("POSTGRES_PASSWORD", POSTGRES_PASSWORD)
        .with_env_var("POSTGRES_DB", POSTGRES_DB)
        // Marks the container as ours, so `task test:clean` can find the ones an interrupted run
        // left behind without touching anything else running on the machine.
        .with_label(TEST_CONTAINER_LABEL, "true")
        .start()
        .await
        .expect("couldn't start Postgres database");

    let postgres_port = postgres_container.get_host_port_ipv4(POSTGRES_PORT)
        .await
        .expect("couldn't fetch port from PostgreSQL server");
    (postgres_container, postgres_port)
}

#[inline]
pub fn get_chat_id_and_dicks(db: &Pool<Postgres>) -> (ChatIdKind, repo::Dicks) {
    let dicks = repo::Dicks::new(db.clone(), Default::default());
    let chat_id = ChatIdKind::ID(TelegramChatId::new(CHAT_ID));
    (chat_id, dicks)
}
