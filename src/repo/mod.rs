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
mod deletions;

#[cfg(test)]
pub(crate) mod test;

use anyhow::anyhow;
use sqlx::{Pool, Postgres};
use sqlx::postgres::PgQueryResult;
pub use users::*;
pub use dicks::*;
pub use chats::*;
pub use import::*;
pub use promo::*;
pub use loans::*;
pub use pvpstats::*;
pub use stats::*;
pub use shrinks::*;
pub use announcements::*;
pub use deletions::*;
use crate::config;
use crate::config::DatabaseConfig;
use crate::domain::primitives::chat::ChatIdKind;

#[derive(Clone)]
pub struct Repositories {
    pub users: Users,
    pub dicks: Dicks,
    pub chats: Chats,
    pub import: Import,
    pub promo: Promo,
    pub loans: Loans,
    pub announcements: Announcements,
    pub pvp_stats: BattleStatsRepo,
    pub personal_stats: PersonalStatsRepo,
    pub shrinks: Shrinks,
    pub deletions: ScheduledDeletions,
}

impl Repositories {
    pub fn new(db_conn: &Pool<Postgres>, config: &config::AppConfig) -> Self {
        Self {
            users: Users::new(db_conn.clone()),
            dicks: Dicks::new(db_conn.clone(), config.features),
            chats: Chats::new(db_conn.clone(), config.features),
            import: Import::new(db_conn.clone()),
            promo: Promo::new(db_conn.clone()),
            loans: Loans::new(db_conn.clone(), config),
            announcements: Announcements::new(db_conn.clone(), config.announcements.clone()),
            pvp_stats: BattleStatsRepo::new(db_conn.clone(), config.features),
            personal_stats: PersonalStatsRepo::new(db_conn.clone()),
            shrinks: Shrinks::new(db_conn.clone()),
            deletions: ScheduledDeletions::new(db_conn.clone()),
        }
    }
}

pub async fn establish_database_connection(config: &DatabaseConfig) -> Result<Pool<Postgres>, anyhow::Error> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .after_connect(|_conn: &mut sqlx::PgConnection, _meta| Box::pin(async move {
            crate::metrics::DB_POOL_CONNECTIONS_OPENED.inc();
            Ok(())
        }))
        .before_acquire(|_conn: &mut sqlx::PgConnection, meta| Box::pin(async move {
            crate::metrics::DB_POOL_IDLE_SECONDS.observe(meta.idle_for.as_secs_f64());
            Ok(true)
        }))
        .after_release(|_conn: &mut sqlx::PgConnection, meta| Box::pin(async move {
            crate::metrics::DB_POOL_CONNECTION_AGE_SECONDS.observe(meta.age.as_secs_f64());
            Ok(true)
        }))
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
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
