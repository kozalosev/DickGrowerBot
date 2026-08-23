//! The list of users who are not allowed to play.
//!
//! A ban is written straight into the database by the owner (see the `erase_user`, `ban_user` and
//! `unban_user` functions in the migrations), and the check runs on every update, so the whole
//! list — a handful of rows at most — is kept in memory rather than asked for each time.
//!
//! The database says when it changes: a trigger on `Users` notifies the `bans` channel, and
//! [`BanList::spawn_listen_task`] reloads on it. The timer stays behind that as the backstop, for
//! the moments a listener is reconnecting and hears nothing.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgListener;
use sqlx::{Pool, Postgres};
use crate::domain::primitives::UserId;
use crate::metrics;
use crate::repo::Users;

/// The channel the trigger of migration 40 announces a changed ban on.
const CHANNEL: &str = "bans";

/// How long to wait before listening again after the connection was lost. The timer keeps the list
/// fresh meanwhile, so there is nothing to be gained by hurrying.
const RECONNECT_DELAY: Duration = Duration::from_mins(1);

type Bans = HashMap<UserId, DateTime<Utc>>;

#[derive(Clone)]
pub struct BanList {
    users: Users,
    bans: Arc<RwLock<Bans>>,
}

impl BanList {
    pub async fn load(users: Users) -> Self {
        let list = Self { users, bans: Arc::new(RwLock::new(Bans::new())) };
        list.refresh().await;
        list
    }

    pub fn banned_until(&self, uid: UserId) -> Option<DateTime<Utc>> {
        self.read()
            .get(&uid)
            .copied()
            .filter(|until| *until > Utc::now())
    }

    /// Reads the list from the database. A failure keeps the previous list
    pub async fn refresh(&self) {
        let banned = match self.users.get_banned().await {
            Ok(banned) => banned,
            Err(e) => {
                tracing::warn!(error = format!("{e:#}"), "couldn't refresh the ban list, keeping the previous one");
                return
            }
        };

        let count = banned.len();
        let bans = banned.into_iter()
            .map(|user| (user.uid, user.banned_until))
            .collect();
        *self.write() = bans;

        tracing::info!(count, "the ban list has been refreshed")
    }

    /// Re-reads the list whenever the database says a user's ban status has changed, which is what
    /// makes one apply at once — on every instance, and without anyone typing a command.
    pub fn spawn_listen_task(&self, pool: Pool<Postgres>) {
        let list = self.clone();
        tokio::spawn(metrics::TASK_BAN_LIST_LISTENER.instrument(async move {
            loop {
                match subscribe(&pool).await {
                    Ok(listener) => consume(&list, listener).await,
                    Err(e) => tracing::error!(error = %e, "couldn't listen for changes to the ban list"),
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }));
    }

    /// Re-reads the list every `interval`. The backstop behind [`Self::spawn_listen_task`]: a
    /// notification sent while the listener was reconnecting is heard by nobody. The owner can also
    /// apply a ban at once by sending SIGHUP — see [`crate::reload::spawn_reload_on_sighup`].
    pub fn spawn_refresh_task(&self, interval: Duration) {
        let list = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.tick().await;  // the first tick fires immediately, and the list is already loaded
            loop {
                ticker.tick().await;
                list.refresh().await;
            }
        });
    }

    fn read(&self) -> RwLockReadGuard<'_, Bans> {
        self.bans.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, Bans> {
        self.bans.write().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Takes a connection and subscribes on it.
async fn subscribe(pool: &Pool<Postgres>) -> sqlx::Result<PgListener> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen(CHANNEL).await?;
    tracing::info!(channel = CHANNEL, "listening for changes to the ban list");
    Ok(listener)
}

/// Refreshes the list on every notification, until the connection is lost.
async fn consume(list: &BanList, mut listener: PgListener) {
    loop {
        match listener.recv().await {
            Ok(notification) => {
                tracing::info!(uid = notification.payload(), "a user's ban status has changed");
                list.refresh().await;
            }
            Err(e) => return tracing::error!(error = %e, "the ban listener has stopped"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::repo::test::{fresh_db, NAME, USER_ID};

    /// How long the listener is given to catch up before the test calls it broken. Generous, and
    /// paid only when something is actually wrong: the loop below returns the moment the list
    /// Only ever spent when the test is about to fail: `recv` returns as soon as the notification
    /// arrives, and a hanging test says less than a failing one.
    const DEADLINE: Duration = Duration::from_secs(10);

    /// The ban is written by hand in the database, so the trigger of migration 40 is the only thing
    /// that can tell the bot about it before the timer comes round. The timer is deliberately not
    /// started here: nothing but the notification may make this pass.
    #[tokio::test]
    async fn a_ban_written_in_the_database_is_heard_without_the_timer() {
        let db = fresh_db().await;
        let users = Users::new(db.clone());
        users.create_or_update(USER_ID, NAME)
            .await.expect("couldn't create the user");

        let list = BanList::load(users).await;
        assert_eq!(list.banned_until(USER_ID), None, "a fresh user must not be banned");

        // Subscribing is what matters, not reading: the ban is written while nothing is polling the
        // connection at all, and has to survive that. What a notification cannot survive is having
        // no LISTEN registered when it is sent — which is why the subscription is taken before the
        // ban, and why the timer still exists for the gap around a reconnect.
        let mut listener = subscribe(&db).await.expect("couldn't subscribe to the ban notifications");

        sqlx::query!("SELECT ban_user($1, 7)", USER_ID as UserId)
            .execute(&db)
            .await.expect("couldn't ban the user");

        let notification = next_notification(&mut listener).await;
        assert_eq!(notification, USER_ID.to_string(), "the payload must name the user");

        // What `consume` does with it, done here so the test can assert without waiting on a task.
        list.refresh().await;
        assert!(list.banned_until(USER_ID).is_some(), "the ban must be in the list");

        sqlx::query!("SELECT unban_user($1)", USER_ID as UserId)
            .execute(&db)
            .await.expect("couldn't unban the user");

        let notification = next_notification(&mut listener).await;
        assert_eq!(notification, USER_ID.to_string(), "lifting a ban must be announced too");
        list.refresh().await;
        assert_eq!(list.banned_until(USER_ID), None, "the ban must be gone from the list");
    }

    /// The next notification, or a failure saying none came. Waits on the thing itself rather than
    /// asking repeatedly whether it has happened yet.
    async fn next_notification(listener: &mut PgListener) -> String {
        tokio::time::timeout(DEADLINE, listener.recv())
            .await.expect("no notification arrived before the deadline")
            .expect("the listener stopped")
            .payload()
            .to_owned()
    }
}
