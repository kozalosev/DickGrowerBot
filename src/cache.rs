//! Values kept in Redis, each with a lifetime of its own.
//!
//! Everything here is a cache and nothing here is a source of truth: a miss is answered by the
//! caller doing the work again, so a Redis that is down, slow or simply not configured costs
//! nothing but the work it would have saved. Every failure is therefore logged and swallowed —
//! this module never returns an error and never panics.
//!
//! It follows that the bot must start and run with `REDIS_HOST` unset, which is what
//! [`Cache::Disabled`] is for. That variant answers "nothing known" to every read and drops every
//! write.

use std::fmt::Display;
use std::time::Duration;
use redis::AsyncTypedCommands;
use redis::aio::ConnectionManager;
use crate::config::RedisConfig;

/// A handle on the cached values. Cheap to clone — the clones share one multiplexed connection.
#[derive(Clone)]
pub enum Cache {
    Disabled,
    Connected(ConnectionManager),
}

impl Cache {
    /// Connects, or stays [`Cache::Disabled`] when Redis isn't configured. A server that is
    /// configured but unreachable disables the cache too, rather than stopping the bot: it holds
    /// nothing the bot can't do without.
    pub async fn connect(config: Option<RedisConfig>) -> Self {
        let Some(config) = config else {
            tracing::info!(enabled = false, "the cache is disabled, no REDIS_HOST is set");
            return Self::Disabled
        };

        match connect_to(config.url).await {
            Ok(conn) => {
                tracing::info!(enabled = true, "the cache is connected");
                Self::Connected(conn)
            }
            Err(e) => {
                tracing::error!(error = %e, "couldn't connect to Redis, the cache stays disabled");
                Self::Disabled
            }
        }
    }

    /// The flag stored under this key, or `None` when nothing is — never written, expired, or the
    /// cache is unavailable. The three are deliberately one answer: every caller falls back the
    /// same way.
    pub async fn get_flag(&self, key: impl CacheKey) -> Option<bool> {
        let Self::Connected(conn) = self else {
            return None
        };
        let key = key.to_string();
        conn.clone().get(&key).await
            .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't read a value from the cache"))
            .ok()
            .flatten()
            .map(|value| value == "1")
    }

    /// Stores a flag for as long as its owner says it stays true.
    ///
    /// The lifetime is a property of the value, not of the store: what makes a chat's language
    /// worth keeping for an hour has nothing to do with what makes a lock worth keeping for
    /// seconds. So it arrives with each write rather than being configured here.
    pub async fn set_flag(&self, key: impl CacheKey, value: bool, ttl: Duration) {
        let Self::Connected(conn) = self else {
            return
        };
        let key = key.to_string();
        let value = if value { "1" } else { "0" };
        conn.clone().set_ex(&key, value, ttl.as_secs()).await
            .unwrap_or_else(|e| tracing::warn!(error = %e, key, "couldn't write a value into the cache"))
    }
}

/// A key in the cache: one type per kind of value, declared by whoever owns that value.
///
/// Everything here shares a single keyspace, so a key's shape is worth a type rather than a
/// `format!` at each call site. The bound is `Display` and not `Into<String>` because that is what
/// stops a bare string being passed off as a key.
pub trait CacheKey: Display {}

async fn connect_to(url: String) -> redis::RedisResult<ConnectionManager> {
    ConnectionManager::new(redis::Client::open(url)?).await
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_containers::SharedContainer;

    /// Stands in for the real keys, which live with the values they name rather than here.
    #[derive(Clone, Copy, derive_more::Display)]
    #[display("test:{_0}")]
    struct TestKey(u8);

    impl CacheKey for TestKey {}

    static CONTAINER: SharedContainer = SharedContainer::new(
        "cache", "valkey/valkey", "9-alpine", 6379, &["Ready to accept connections"]);

    #[tokio::test]
    async fn a_value_survives_a_round_trip() {
        let cache = cache().await;
        let key = TestKey(1);

        cache.set_flag(key, true, A_MINUTE).await;
        assert_eq!(cache.get_flag(key).await, Some(true));

        cache.set_flag(key, false, A_MINUTE).await;
        assert_eq!(cache.get_flag(key).await, Some(false));
    }

    #[tokio::test]
    async fn an_absent_key_is_nothing_known() {
        let cache = cache().await;
        assert_eq!(cache.get_flag(TestKey(2)).await, None);
    }

    #[tokio::test]
    async fn a_value_stops_being_known_once_its_time_is_up() {
        let cache = cache().await;
        let key = TestKey(3);

        cache.set_flag(key, false, Duration::from_secs(1)).await;
        assert_eq!(cache.get_flag(key).await, Some(false));

        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(cache.get_flag(key).await, None);
    }

    #[tokio::test]
    async fn a_disabled_cache_knows_nothing_and_keeps_nothing() {
        let cache = Cache::Disabled;

        cache.set_flag(TestKey(4), true, A_MINUTE).await;
        assert_eq!(cache.get_flag(TestKey(4)).await, None);
    }

    #[tokio::test]
    async fn an_unreachable_server_disables_the_cache() {
        // Port 1 is never a Redis; the bot must start anyway.
        let cache = Cache::connect(Some(RedisConfig { url: "redis://localhost:1/".to_owned() })).await;
        assert!(matches!(cache, Cache::Disabled));
    }

    /// A connected cache. The whole test binary shares one server, so a test that needs isolation
    /// asks for a key of its own.
    async fn cache() -> Cache {
        let port = CONTAINER.port().await;
        Cache::connect(Some(RedisConfig { url: format!("redis://localhost:{port}/") })).await
    }

    const A_MINUTE: Duration = Duration::from_secs(60);
}
