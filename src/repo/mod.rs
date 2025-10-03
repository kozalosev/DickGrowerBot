mod users;
mod dicks;
mod chats;
mod promo;
mod loans;
mod pvpstats;
mod stats;
mod announcements;

#[cfg(test)]
pub(crate) mod test;

use anyhow::anyhow;
use sqlx::{Pool, Postgres};
use sqlx::postgres::PgQueryResult;
use teloxide::types::{ChatId, UserId};
pub use users::*;
pub use dicks::*;
pub use chats::*;
pub use promo::*;
pub use loans::*;
pub use pvpstats::*;
pub use stats::*;
pub use announcements::*;
use crate::config;
use crate::config::DatabaseConfig;

#[derive(Clone)]
pub struct Repositories {
    pub users: Users,
    pub dicks: Dicks,
    pub chats: Chats,
    pub promo: Promo,
    pub loans: Loans,
    pub announcements: Announcements,
    pub pvp_stats: BattleStatsRepo,
    pub personal_stats: PersonalStatsRepo,
}

impl Repositories {
    pub fn new(db_conn: &Pool<Postgres>, config: &config::AppConfig) -> Self {
        Self {
            users: Users::new(db_conn.clone()),
            dicks: Dicks::new(db_conn.clone(), config.features),
            chats: Chats::new(db_conn.clone(), config.features),
            promo: Promo::new(db_conn.clone()),
            loans: Loans::new(db_conn.clone(), config),
            announcements: Announcements::new(db_conn.clone(), config.announcements.clone()),
            pvp_stats: BattleStatsRepo::new(db_conn.clone(), config.features),
            personal_stats: PersonalStatsRepo::new(db_conn.clone()),
        }
    }
}

#[derive(derive_more::Display, Debug, Default, Copy, Clone)]
pub enum ChatIdSource {
    InlineQuery,
    #[default] Database,
}

#[derive(derive_more::Display, Debug, Clone)]
pub enum ChatIdPartiality {
    #[display("ChatIdPartiality::Both({_0}, {_1})")]
    Both(ChatIdFull, ChatIdSource),
    #[display("ChatIdPartiality::Specific({_0})")]
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
            ChatIdPartiality::Both(ChatIdFull { id, instance }, qs) => match qs {
                ChatIdSource::Database => ChatIdKind::ID(*id),
                ChatIdSource::InlineQuery => ChatIdKind::Instance(instance.clone()),
            }
            ChatIdPartiality::Specific(kind) => kind.clone()
        }
    }
}

#[derive(Debug, Clone, derive_more::Display)]
#[display("ChatIdFull({id}, {instance})")]
pub struct ChatIdFull {
    pub id: ChatId,
    pub instance: String,
}

impl ChatIdFull {
    #[allow(clippy::wrong_self_convention)]
    pub fn to_partiality(self, query_source: ChatIdSource) -> ChatIdPartiality {
        ChatIdPartiality::Both(self, query_source)
    }
}

#[derive(Debug, derive_more::Display, Clone, Eq, PartialEq, Hash)]
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

#[derive(Debug, Copy, Clone, sqlx::Type)]
#[sqlx(transparent)]
pub struct ChatIdInternal(i64);

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, derive_more::From)]
pub struct UID(i64);

impl From<UserId> for UID {
    fn from(value: UserId) -> Self {
        Self(value.0 as i64)
    }
}

#[allow(clippy::from_over_into)]
impl Into<UserId> for UID {
    fn into(self) -> UserId {
        UserId(self.0 as u64)
    }
}


pub async fn establish_database_connection(config: &DatabaseConfig) -> Result<Pool<Postgres>, anyhow::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(config.max_connections)
        .connect(config.url.as_str()).await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}


#[macro_export]
macro_rules! repository {
    ($name:ident, with_feature_toggles, $($methods:item),*) => {
        #[derive(Clone)]
        pub struct $name {
            pool: sqlx::Pool<sqlx::Postgres>,
            features: $crate::config::FeatureToggles,
        }

        impl $name {
            pub fn new(pool: sqlx::Pool<sqlx::Postgres>, features: $crate::config::FeatureToggles) -> Self {
                Self { pool, features }
            }

            $($methods)*
        }
    };
    
    ($name:ident, with_($repoName:ident)_($repoType:tt), $($methods:item),*) => {
        #[derive(Clone)]
        pub struct $name {
            pool: sqlx::Pool<sqlx::Postgres>,
            #[allow(dead_code)] features: $crate::config::FeatureToggles,
            $repoName: $crate::repo::$repoType,
        }

        impl $name {
            pub fn new(pool: sqlx::Pool<sqlx::Postgres>, features: $crate::config::FeatureToggles) -> Self {
                let inner_repo = $crate::repo::$repoType::new(pool.clone(), features);
                Self { pool, features, $repoName: inner_repo }
            }

            $($methods)*
        }
    };
    
    ($name:ident, $($methods:item),*) => {
        #[derive(Clone)]
        pub struct $name {
            pool: sqlx::Pool<sqlx::Postgres>,
        }

        impl $name {
            pub fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
                Self { pool }
            }

            $($methods)*
        }
    };
}

fn ensure_only_one_row_updated(res: PgQueryResult) -> Result<(), anyhow::Error> {
    match res.rows_affected() {
        1 => Ok(res),
        x => Err(anyhow!("not only one row was updated but {x}"))
    }.map(|_| ())
}
