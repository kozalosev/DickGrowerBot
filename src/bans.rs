//! The list of users who are not allowed to play.
//!
//! A ban is written straight into the database by the owner (see the `erase_user`, `ban_user` and
//! `unban_user` functions in the migrations), so the bot has to poll for it. The list is small —
//! a handful of rows at most — so the whole of it is kept in memory and refreshed on a timer,
//! instead of asking the database on every single update.

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;
use chrono::{DateTime, Utc};
use crate::domain::primitives::UserId;
use crate::repo::Users;

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

    /// Re-reads the list every `interval`. The owner can also apply a ban at once by sending
    /// SIGHUP — see [`crate::reload::spawn_reload_on_sighup`].
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
