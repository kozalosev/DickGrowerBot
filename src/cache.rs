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
    Connected {
        conn: ConnectionManager,
        ttl: Duration,
    },
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
                tracing::info!(enabled = true, ttl_secs = config.cache_ttl.as_secs(), "the cache is connected");
                Self::Connected { conn, ttl: config.cache_ttl }
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
        let Self::Connected { conn, .. } = self else {
            return None
        };
        let key = key.to_string();
        conn.clone().get(&key).await
            .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't read a value from the cache"))
            .ok()
            .flatten()
            .map(|value| value == "1")
    }

    /// Stores a flag for as long as the configured lifetime.
    pub async fn set_flag(&self, key: impl CacheKey, value: bool) {
        let Self::Connected { conn, ttl } = self else {
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

#[inline]
async fn connect_to(url: String) -> redis::RedisResult<ConnectionManager> {
    ConnectionManager::new(redis::Client::open(url)?).await
}

#[cfg(test)]
mod test {
    use super::*;
    use once_cell::sync::Lazy;
    use testcontainers::{ContainerAsync, GenericImage, ImageExt, ReuseDirective};
    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::runners::AsyncRunner;
    use tokio::runtime::{Builder, Runtime};
    use tokio::sync::OnceCell;
    use crate::repo::test::TEST_CONTAINER_LABEL;

    /// Stands in for the real keys, which live with the values they name rather than here.
    #[derive(Clone, Copy, derive_more::Display)]
    #[display("test:{_0}")]
    struct TestKey(u8);

    impl CacheKey for TestKey {}

    const IMAGE: &str = "valkey/valkey";
    const TAG: &str = "9-alpine";
    const PORT: u16 = 6379;

    /// The same arrangement the database tests use: a sqlx pool dies with the runtime that made it,
    /// and so does a `ConnectionManager`, so the shared container lives on a runtime of its own that
    /// outlives every test. See `repo::test` for the full reasoning.
    static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("couldn't build the shared cache-test runtime")
    });

    static CONTAINER: OnceCell<(ContainerAsync<GenericImage>, u16)> = OnceCell::const_new();

    async fn start() -> (ContainerAsync<GenericImage>, u16) {
        let container = GenericImage::new(IMAGE, TAG)
            .with_exposed_port(PORT.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .with_label(TEST_CONTAINER_LABEL, "cache")
            .with_reuse(ReuseDirective::Always)
            .start()
            .await
            .expect("couldn't start the cache container");
        let port = container.get_host_port_ipv4(PORT)
            .await
            .expect("couldn't fetch the port of the cache container");
        (container, port)
    }

    /// A connected cache with the given TTL. The whole test binary shares one server, so a test
    /// that needs isolation asks for a key of its own.
    async fn cache(ttl: Duration) -> Cache {
        let port = RUNTIME
            .spawn(async { CONTAINER.get_or_init(start).await.1 })
            .await
            .expect("the shared cache container task failed");
        Cache::connect(Some(RedisConfig {
            url: format!("redis://localhost:{port}/"),
            cache_ttl: ttl,
        })).await
    }

    #[tokio::test]
    async fn a_value_survives_a_round_trip() {
        let cache = cache(Duration::from_secs(60)).await;
        let key = TestKey(1);

        cache.set_flag(key, true).await;
        assert_eq!(cache.get_flag(key).await, Some(true));

        cache.set_flag(key, false).await;
        assert_eq!(cache.get_flag(key).await, Some(false));
    }

    #[tokio::test]
    async fn an_absent_key_is_nothing_known() {
        let cache = cache(Duration::from_secs(60)).await;
        assert_eq!(cache.get_flag(TestKey(2)).await, None);
    }

    #[tokio::test]
    async fn a_value_stops_being_known_once_its_time_is_up() {
        let cache = cache(Duration::from_secs(1)).await;
        let key = TestKey(3);

        cache.set_flag(key, false).await;
        assert_eq!(cache.get_flag(key).await, Some(false));

        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(cache.get_flag(key).await, None);
    }

    #[tokio::test]
    async fn a_disabled_cache_knows_nothing_and_keeps_nothing() {
        let cache = Cache::Disabled;

        cache.set_flag(TestKey(4), true).await;
        assert_eq!(cache.get_flag(TestKey(4)).await, None);
    }

    #[tokio::test]
    async fn an_unreachable_server_disables_the_cache() {
        // Port 1 is never a Redis; the bot must start anyway.
        let cache = Cache::connect(Some(RedisConfig {
            url: "redis://localhost:1/".to_owned(),
            cache_ttl: Duration::from_secs(60),
        })).await;
        assert!(matches!(cache, Cache::Disabled));
    }
}
