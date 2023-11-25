mod users;
mod dicks;
mod import;
mod promo;

#[cfg(test)]
pub(crate) mod test;

use strum_macros::Display;
use teloxide::types::ChatId;
pub use users::*;
pub use dicks::*;
pub use import::*;
pub use promo::*;
use crate::config::DatabaseConfig;

#[derive(Clone)]
pub struct Repositories {
    pub users: Users,
    pub dicks: Dicks,
    pub import: Import,
    pub promo: Promo,
}

#[derive(Display, Debug)]
pub enum ChatIdPartiality {
    Both(ChatIdFull),
    Specific(ChatIdKind)
}

impl From<ChatId> for ChatIdPartiality {
    fn from(value: ChatId) -> Self {
        Self::Specific(ChatIdKind::ID(value))
    }
}

impl From<String> for ChatIdPartiality {
    fn from(value: String) -> Self {
        Self::Specific(ChatIdKind::Instance(value))
    }
}

impl From<ChatIdKind> for ChatIdPartiality {
    fn from(value: ChatIdKind) -> Self {
        Self::Specific(value)
    }
}

impl ChatIdPartiality {
    pub fn kind(&self) -> ChatIdKind {
        match self {
            ChatIdPartiality::Both(ChatIdFull { id, .. }) => ChatIdKind::ID(*id),
            ChatIdPartiality::Specific(kind) => kind.clone()
        }
    }
}

impl From<ChatIdFull> for ChatIdPartiality {
    fn from(value: ChatIdFull) -> Self {
        Self::Both(value)
    }
}

#[derive(Debug, Clone)]
pub struct ChatIdFull {
    pub id: ChatId,
    pub instance: String,
}

#[derive(Debug, Display, Clone)]
pub enum ChatIdKind {
    ID(ChatId),
    Instance(String)
}

impl From<ChatId> for ChatIdKind {
    fn from(value: ChatId) -> Self {
        ChatIdKind::ID(value)
    }
}

impl From<String> for ChatIdKind {
    fn from(value: String) -> Self {
        ChatIdKind::Instance(value)
    }
}

impl ChatIdKind {
    pub fn value(&self) -> String {
        match self {
            ChatIdKind::ID(id) => id.0.to_string(),
            ChatIdKind::Instance(instance) => instance.to_owned(),
        }
    }
}

#[derive(sqlx::Type)]
#[sqlx(type_name = "chat_id_type")]
#[sqlx(rename_all = "lowercase")]
enum ChatIdType {
    ID,
    Inst,
}

impl From<&ChatIdKind> for ChatIdType {
    fn from(value: &ChatIdKind) -> Self {
        match value {
            ChatIdKind::ID(_) => ChatIdType::ID,
            ChatIdKind::Instance(_) => ChatIdType::Inst,
        }
    }
}


pub async fn establish_database_connection(config: &DatabaseConfig) -> Result<sqlx::Pool<sqlx::Postgres>, anyhow::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(config.url.as_str()).await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}


#[macro_export]
macro_rules! repository {
    ($name:ident, $($methods:item),*) => {
        #[derive(Clone)]
        pub struct $name {
            pool: sqlx::Pool<sqlx::Postgres>
        }

        impl $name {
            pub fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
                Self { pool }
            }

            $($methods)*
        }
    };
}
