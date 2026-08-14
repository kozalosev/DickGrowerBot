//! Keeping two answers to the same battle offer from being processed at once.
//!
//! The lock goes through the store rather than a set of this process's own. One instance of the bot
//! is served either way; the difference only shows with a second, which would see nothing of the
//! first one's locks and accept an attack it was already resolving.
//!
//! The value under the key is the token of whoever holds it, which is what makes the release safe:
//! a handler that outran the lifetime has already lost the lock, and must not take away the hold
//! that replaced it.
//!
//! There is no way to switch this off. It is not a feature but a guard against answering one offer
//! twice, and the store it needs is always there: [`Cache::or_local`] gives one even where an
//! operator turned the caching off.

use std::time::Duration;
use derive_more::Display;
use crate::cache::{Cache, CacheKey, LockToken};
use crate::handlers::utils::callbacks::CallbackDataWithPrefix;
use crate::metrics;

/// Keyed by the offer, because that is what may only be answered once.
#[derive(Clone, Display)]
#[display("lock:pvp:{_0}")]
pub struct BattleLockKey(String);

impl CacheKey for BattleLockKey {}

#[derive(Clone)]
pub struct BattleLocks {
    cache: Cache,
    ttl: Duration,
}

impl BattleLocks {
    pub fn new(cache: &Cache, ttl: Duration) -> Self {
        Self { cache: cache.or_local(), ttl }
    }

    /// Takes the lock on this battle, or answers `None` when somebody else holds it.
    pub async fn try_lock<T>(&self, callback_data: &T) -> Option<BattleGuard>
    where T: CallbackDataWithPrefix,
    {
        let key = BattleLockKey(callback_data.to_string());
        match self.cache.lock(key.clone(), self.ttl).await {
            Some(token) => {
                tracing::debug!(key = %key, "taking a lock guard");
                Some(BattleGuard { cache: self.cache.clone(), key, token })
            }
            None => {
                metrics::PVP_DOUBLE_ATTACKS_BLOCKED.inc();
                tracing::debug!(key = %key, "a double attack was blocked");
                None
            }
        }
    }
}

/// Frees the lock when the handler is done with it. The lifetime the key was taken for is only the
/// backstop for a guard that never gets to run — a process killed mid-battle.
///
/// It frees its own hold and not merely the key. A handler that outlives the lifetime has already
/// lost the lock to whoever asked next, and taking that one away would let a third answer in while
/// the second is still being resolved — the very thing the guard is here for.
pub struct BattleGuard {
    cache: Cache,
    key: BattleLockKey,
    token: LockToken,
}

impl Drop for BattleGuard {
    fn drop(&mut self) {
        tracing::debug!(key = %self.key, "dropping the lock guard");
        let (cache, key, token) = (self.cache.clone(), self.key.clone(), self.token.clone());
        tokio::spawn(async move {
            if !cache.unlock(key.clone(), &token).await {
                tracing::warn!(key = %key, "the battle outlived its lock, so somebody else may be resolving it");
            }
        });
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::config::{CacheConfig, CacheMode};
    use crate::handlers::utils::callbacks::InvalidCallbackData;

    #[derive(Display)]
    #[display("{_0}")]
    struct TestCallbackData(u8);

    impl TryFrom<String> for TestCallbackData {
        type Error = InvalidCallbackData;

        fn try_from(value: String) -> Result<Self, Self::Error> {
            value.parse().map(Self).map_err(|_| InvalidCallbackData::NoData)
        }
    }

    impl CallbackDataWithPrefix for TestCallbackData {
        fn prefix() -> &'static str {
            "test"
        }
    }

    /// Two services over one store stand in for two instances of the bot — the case a set held per
    /// process could not serve, and the reason the lock moved into the store.
    #[tokio::test]
    async fn one_battle_is_answered_once_however_many_instances_ask() {
        let (first, second) = two_instances(CacheMode::Local).await;
        let battle = TestCallbackData(1);

        let guard = first.try_lock(&battle).await;
        assert!(guard.is_some());
        assert!(second.try_lock(&battle).await.is_none());
    }

    #[tokio::test]
    async fn the_battle_is_free_again_once_the_guard_is_dropped() {
        let (first, second) = two_instances(CacheMode::Local).await;
        let battle = TestCallbackData(2);

        drop(first.try_lock(&battle).await);
        // The guard frees the key from a task of its own, since dropping can't await.
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(second.try_lock(&battle).await.is_some());
    }

    /// Another battle is another key, so an attack on one never blocks the other.
    #[tokio::test]
    async fn two_battles_do_not_block_each_other() {
        let (locks, _) = two_instances(CacheMode::Local).await;

        let _first = locks.try_lock(&TestCallbackData(3)).await.expect("the first battle must be free");
        assert!(locks.try_lock(&TestCallbackData(4)).await.is_some());
    }

    /// Turning the caching off must not turn this off with it: a battle answered twice is the bug
    /// the lock exists for, not a setting.
    #[tokio::test]
    async fn a_disabled_cache_still_locks() {
        let (first, second) = two_instances(CacheMode::Disabled).await;
        let battle = TestCallbackData(5);

        assert!(first.try_lock(&battle).await.is_some());
        assert!(second.try_lock(&battle).await.is_none());
    }

    /// Two services over one store, which is what two instances of the bot amount to.
    async fn two_instances(mode: CacheMode) -> (BattleLocks, BattleLocks) {
        let cache = Cache::connect(CacheConfig { mode, url: None }).await;
        let ttl = Duration::from_secs(30);
        (BattleLocks::new(&cache, ttl), BattleLocks::new(&cache, ttl))
    }
}
