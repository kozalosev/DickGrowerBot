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
mod broadcasts;
mod perks;
mod customizations;

#[cfg(test)]
pub(crate) mod test;

use anyhow::anyhow;
use sqlx::{Pool, Postgres};
use sqlx::postgres::{PgConnectOptions, PgQueryResult};
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
pub use broadcasts::*;
pub use perks::*;
pub use customizations::*;
use crate::config;
use crate::config::DatabaseConfig;
use crate::domain::primitives::chat::ChatIdKind;

/// Connections the pool holds on top of what queries may use. A listening connection can't serve
/// queries, so it is held for as long as it listens; the pool is built that much larger, and
/// `DATABASE_MAX_CONNECTIONS` keeps meaning what an operator set it to.
const LISTENER_CONNECTIONS: u32 = 1;

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
    pub broadcasts: ScheduledBroadcasts,
    pub perk_states: PerkStates,
    pub customizations: Customizations,
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
            broadcasts: ScheduledBroadcasts::new(db_conn.clone()),
            perk_states: PerkStates::new(db_conn.clone()),
            customizations: Customizations::new(db_conn.clone()),
        }
    }
}

pub async fn establish_database_connection(config: &DatabaseConfig) -> Result<Pool<Postgres>, anyhow::Error> {
    migrate(config).await?;

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
        .max_connections(config.max_connections + LISTENER_CONNECTIONS)
        .min_connections(config.min_connections)
        .acquire_timeout(config.acquire_timeout)
        .connect(config.url.as_str()).await?;
    Ok(pool)
}

/// Brings the schema up to date through a pool of its own, thrown away as soon as it is done.
///
/// A `statement_timeout` in `DATABASE_URL` bounds every connection made from it, and a migration is
/// the one thing that must not be bounded: building an index over millions of rows is meant to take
/// minutes, and a schema change cut off halfway is worse than any slow query. A `CREATE INDEX
/// CONCURRENTLY` can't even lift the limit for itself — it refuses to run in a transaction block,
/// which is what a multi-statement query string becomes.
async fn migrate(config: &DatabaseConfig) -> anyhow::Result<()> {
    // Postgres takes the last of the repeated options, so this undoes whatever the URL asked for.
    let connect_options: PgConnectOptions = config.url.as_str().parse()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options.options([("statement_timeout", "0")])).await?;
    sqlx::migrate!().run(&pool).await?;
    pool.close().await;
    Ok(())
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
