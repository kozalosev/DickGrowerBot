mod users;
mod dicks;
mod chats;
mod import;
mod promo;

use std::str::FromStr;
use reqwest::Url;
use sqlx::{Pool, Postgres};
use teloxide::types::ChatId;
use testcontainers::{clients, Container, GenericImage};
use testcontainers::core::WaitFor;
use crate::config::DatabaseConfig;
use crate::repo;
use crate::repo::ChatIdKind;

const POSTGRES_USER: &str = "test";
const POSTGRES_PASSWORD: &str = "test_pw";
const POSTGRES_DB: &str = "test_db";
const POSTGRES_PORT: u16 = 5432;

pub const UID: i64 = 12345;
pub const CHAT_ID: i64 = 67890;
pub const NAME: &str = "test";

pub async fn start_postgres(docker: &clients::Cli) -> (Container<GenericImage>, Pool<Postgres>) {
    let postgres_image = GenericImage::new("postgres", "latest")
        .with_exposed_port(POSTGRES_PORT)
        .with_wait_for(WaitFor::message_on_stdout("PostgreSQL init process complete; ready for start up."))
        .with_env_var("POSTGRES_USER", POSTGRES_USER)
        .with_env_var("POSTGRES_PASSWORD", POSTGRES_PASSWORD)
        .with_env_var("POSTGRES_DB", POSTGRES_DB);

    let postgres_container = docker.run(postgres_image);
    let postgres_port = postgres_container.get_host_port_ipv4(POSTGRES_PORT);
    let db_url = Url::from_str(&format!("postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{postgres_port}/{POSTGRES_DB}"))
        .expect("invalid database URL");
    let conf = DatabaseConfig{
        url: db_url,
        max_connections: 5,
    };
    let pool = repo::establish_database_connection(&conf)
        .await.expect("couldn't establish a database connection");
    (postgres_container, pool)
}

#[inline]
pub fn get_chat_id_and_dicks(db: &Pool<Postgres>) -> (ChatIdKind, repo::Dicks) {
    let dicks = repo::Dicks::new(db.clone());
    let chat_id = ChatIdKind::ID(ChatId(CHAT_ID));
    (chat_id, dicks)
}
