//! The values the bot keeps briefly, each with a lifetime of its own.
//!
//! Almost everything here is a cache and almost nothing here is a source of truth: a miss is
//! answered by the caller doing the work again, so a Redis that is down, slow or simply not
//! configured costs nothing but the work it would have saved. Every failure is therefore logged
//! and swallowed — this module never returns an error and never panics.
//!
//! It follows that the bot must start and run with `REDIS_HOST` unset, which is what
//! [`Backend::Local`] is for: the same keyspace, kept in this process. [`Backend::Disabled`] is the
//! one an operator chooses, and it answers "nothing known" to every read and drops every write.
//!
//! The exceptions are the two tenants a miss is *not* free for — a lock nobody holds is no lock,
//! and a dialogue with nowhere to keep its state can never advance. They ask for
//! [`Cache::or_local`], which turns a disabled store into a local one and leaves the rest alone.

use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use once_cell::sync::Lazy;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::time::Instant;
use domain_types::traits::SaturatingInto;
use crate::config::{CacheConfig, CacheMode};
use crate::metrics;
use crate::metrics::CacheSourceCounters;

/// How often the local store is swept of the entries nobody came back for.
///
/// Deliberately a constant where the rest of the store's numbers are environment variables: every
/// answer the store gives is the same whatever this is set to, because the read path checks the
/// deadline itself and an expired entry is already unreadable. All it decides is how promptly the
/// memory behind one is handed back.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// What every key of this bot's begins with.
///
/// A server may be shared, and a key like `chat:id:-1001234:language` says nothing about whose chat
/// that is. Applied here rather than left to each [`CacheKey`], so that a kind of value added later
/// cannot be the one that forgets. Isolation by database index in the URL still works, and the two
/// together mean neither has to be relied on alone.
const KEY_PREFIX: &str = "dgb";

const TRUE: &[u8] = b"1";
const FALSE: &[u8] = b"0";

/// Frees a lock only if it is still the one that was taken.
///
/// A bare `DEL` frees whatever holds the key, and after an expiry that may be somebody else's
/// lock — freeing which lets a third caller in while the second is still working. The check and the
/// delete therefore have to happen together, and two commands can't: `MULTI`/`EXEC` is unsafe on a
/// multiplexed connection. A script is one command to the server, so it is as safe here as
/// `SET NX EX` is, and it is what the two steps are sent as.
static UNLOCK: Lazy<redis::Script> = Lazy::new(|| redis::Script::new(
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end"));

/// A handle on the kept values. Cheap to clone — the clones share one multiplexed connection, or
/// one local store.
#[derive(Clone)]
pub struct Cache {
    backend: Backend,
}

impl Cache {
    /// Connects, or keeps the values in this process when Redis isn't configured. A server that is
    /// configured but unreachable falls back the same way rather than stopping the bot: it holds
    /// nothing the bot can't do without.
    pub async fn connect(config: CacheConfig) -> Self {
        let backend = match config.mode {
            CacheMode::Disabled => {
                tracing::info!(mode = %config.mode, "nothing is cached");
                Backend::Disabled(LocalStore::default())
            }
            CacheMode::Local => {
                tracing::info!(mode = %config.mode, "the cached values are kept in this process");
                Backend::Local(LocalStore::default())
            }
            CacheMode::Redis => connect_to_redis(config.url).await,
        };
        if let Backend::Disabled(store) | Backend::Local(store) = &backend {
            store.spawn_sweeper();
        }
        // Keeping the values here is a setting, and the only one a single instance of the bot
        // needs; being told to share them and failing to is a fault. Only the second is worth
        // waking anybody for, so it is the one with a number of its own.
        let fell_back = matches!(config.mode, CacheMode::Redis) && !matches!(backend, Backend::Redis(_));
        metrics::CACHE_FALLBACK_ACTIVE.set(fell_back.into());
        Self { backend }
    }

    /// The same store for a tenant that can't be turned off. A disabled cache becomes a local one —
    /// the store it was carrying for exactly this — and everything else is already what it should
    /// be.
    pub fn or_local(&self) -> Self {
        let backend = match &self.backend {
            Backend::Disabled(store) => Backend::Local(store.clone()),
            backend => backend.clone(),
        };
        Self { backend }
    }

    /// The flag stored under this key, or `None` when nothing is — never written, expired, or the
    /// cache is unavailable. The three are deliberately one answer: every caller falls back the
    /// same way.
    pub async fn get_flag(&self, key: impl CacheKey) -> Option<bool> {
        self.get_bytes(key).await.map(|bytes| bytes == TRUE)
    }

    /// Stores a flag for as long as its owner says it stays true.
    ///
    /// The lifetime is a property of the value, not of the store: what makes a chat's language
    /// worth keeping for an hour has nothing to do with what makes a lock worth keeping for
    /// seconds. So it arrives with each write rather than being configured here.
    pub async fn set_flag(&self, key: impl CacheKey, value: bool, ttl: Duration) {
        let value = if value { TRUE } else { FALSE };
        self.set_bytes(key, value.to_vec(), ttl).await
    }

    /// Stores a value through serde, for a write-through that has nothing to read back.
    pub async fn set_json<T: Serialize>(&self, key: impl CacheKey, value: &T, ttl: Duration) {
        self.set_json_at(&full_key(key), value, ttl).await
    }

    /// The counterpart of [`Cache::set_json`], for a test that wants to see what was written.
    /// Nothing in the bot reads that way — every reader goes through [`Cache::read_through`].
    #[cfg(test)]
    pub async fn get_json<T: DeserializeOwned>(&self, key: impl CacheKey) -> Option<T> {
        self.get_json_at(&full_key(key)).await
    }

    pub async fn get_bytes(&self, key: impl CacheKey) -> Option<Vec<u8>> {
        self.get_raw(&full_key(key)).await
    }

    pub async fn set_bytes(&self, key: impl CacheKey, value: Vec<u8>, ttl: Duration) {
        self.set_raw(&full_key(key), value, ttl).await
    }

    /// Forgets the value, and says whether there was one.
    pub async fn remove(&self, key: impl CacheKey) -> bool {
        let key = full_key(key);
        match &self.backend {
            Backend::Disabled(_) => false,
            Backend::Local(store) => store.remove(&key),
            Backend::Redis(conn) => conn.clone().del::<_, usize>(&key).await
                .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't remove a value from the cache"))
                .is_ok_and(|removed| removed > 0),
        }
    }

    /// Takes the key for `ttl`, answering with the token that holds it or `None` when somebody else
    /// does. Has to be atomic, since two callers asking at once must not both be told yes.
    ///
    /// A disabled store never holds anything, so it always answers with a token — which is why the
    /// tenants that lock ask for [`Cache::or_local`] first.
    pub async fn lock(&self, key: impl CacheKey, ttl: Duration) -> Option<LockToken> {
        let key = full_key(key);
        let token = LockToken::new();
        let taken = match &self.backend {
            Backend::Disabled(_) => true,
            Backend::Local(store) => store.insert_if_absent(&key, token.0.clone().into_bytes(), ttl),
            Backend::Redis(conn) => {
                let options = redis::SetOptions::default()
                    .conditional_set(redis::ExistenceCheck::NX)
                    .with_expiration(redis::SetExpiry::EX(ttl.as_secs()));
                conn.clone().set_options::<_, _, Option<String>>(&key, &token.0, options).await
                    .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't take a lock, letting the caller through"))
                    // An unreachable server must not stop the bot working, so a failure counts as
                    // free — the same fallback a miss gets everywhere else here.
                    .map_or(true, |answer| answer.is_some())
            }
        };
        taken.then_some(token)
    }

    /// Frees a lock, and says whether this token was the one still holding it.
    ///
    /// `false` means the lock had already run out and been taken by somebody else, whose hold is
    /// left alone — see [`UNLOCK`] for why that has to be decided by the store rather than here.
    pub async fn unlock(&self, key: impl CacheKey, token: &LockToken) -> bool {
        let key = full_key(key);
        match &self.backend {
            Backend::Disabled(_) => false,
            Backend::Local(store) => store.remove_if_holds(&key, token.0.as_bytes()),
            Backend::Redis(conn) => UNLOCK.key(&key).arg(&token.0)
                .invoke_async::<usize>(&mut conn.clone()).await
                .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't free a lock, leaving it to run out"))
                .is_ok_and(|freed| freed > 0),
        }
    }

    /// The value under this key, from the cache when it is there and from `load` when it isn't.
    ///
    /// `counters` say where each lookup was answered from. They are passed in rather than named
    /// here: which caches exist is none of this module's business.
    pub async fn read_through<T, F, Fut>(
        &self,
        key: impl CacheKey,
        ttl: Duration,
        counters: &CacheSourceCounters,
        load: F,
    ) -> T
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let key = full_key(key);
        if let Some(cached) = self.get_json_at(&key).await {
            counters.cache_hit();
            return cached;
        }

        counters.db_query();
        let value = load().await;
        self.set_json_at(&key, &value, ttl).await;
        value
    }

    /// A value that no longer deserializes — the type changed since it was written — counts as
    /// absent, which is the answer every miss here gives.
    async fn get_json_at<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let bytes = self.get_raw(key).await?;
        serde_json::from_slice(&bytes)
            .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't read a value out of the cache"))
            .ok()
    }

    async fn set_json_at<T: Serialize>(&self, key: &str, value: &T, ttl: Duration) {
        match serde_json::to_vec(value) {
            Ok(bytes) => self.set_raw(key, bytes, ttl).await,
            Err(e) => tracing::warn!(error = %e, key, "couldn't write a value into the cache"),
        }
    }

    async fn get_raw(&self, key: &str) -> Option<Vec<u8>> {
        match &self.backend {
            Backend::Disabled(_) => None,
            Backend::Local(store) => store.get(key),
            Backend::Redis(conn) => conn.clone().get::<_, Option<Vec<u8>>>(key).await
                .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't read a value from the cache"))
                .ok()
                .flatten(),
        }
    }

    async fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Duration) {
        match &self.backend {
            Backend::Disabled(_) => {}
            Backend::Local(store) => store.insert(key, value, ttl),
            Backend::Redis(conn) => conn.clone().set_ex::<_, _, ()>(key, value, ttl.as_secs()).await
                .unwrap_or_else(|e| tracing::warn!(error = %e, key, "couldn't write a value into the cache")),
        }
    }
}

/// A key in the cache: one type per kind of value, declared by whoever owns that value.
///
/// Everything here shares a single keyspace, so a key's shape is worth a type rather than a
/// `format!` at each call site. The bound is `Display` and not `Into<String>` because that is what
/// stops a bare string being passed off as a key.
pub trait CacheKey: Display {}

/// What a held lock is worth: proof that the hold being freed is the one that was taken, and not a
/// later one that replaced it after an expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockToken(String);

impl LockToken {
    fn new() -> Self {
        Self(rand::random::<u128>().to_string())
    }
}

#[derive(Clone)]
enum Backend {
    /// Keeps nothing and answers nothing — but still carries a store, because that is what
    /// [`Cache::or_local`] hands to the two tenants an operator's choice may not reach.
    Disabled(LocalStore),
    Local(LocalStore),
    Redis(ConnectionManager),
}

/// The values kept in this process, in one map for every kind of them, so that there is one sweeper
/// and one number to watch however many tenants there are.
#[derive(Clone, Default)]
struct LocalStore(Arc<Mutex<HashMap<String, Entry>>>);

struct Entry {
    bytes: Vec<u8>,
    until: Instant,
}

impl LocalStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let now = Instant::now();
        self.entries()
            .get(key)
            .filter(|entry| entry.until > now)
            .map(|entry| entry.bytes.clone())
    }

    fn insert(&self, key: &str, bytes: Vec<u8>, ttl: Duration) {
        self.entries().insert(key.to_owned(), Entry::new(bytes, ttl));
    }

    /// Takes the key only if it is free, which an expired entry counts as.
    fn insert_if_absent(&self, key: &str, bytes: Vec<u8>, ttl: Duration) -> bool {
        let now = Instant::now();
        let mut entries = self.entries();
        match entries.get(key) {
            Some(entry) if entry.until > now => false,
            _ => {
                entries.insert(key.to_owned(), Entry::new(bytes, ttl));
                true
            }
        }
    }

    fn remove(&self, key: &str) -> bool {
        let now = Instant::now();
        self.entries()
            .remove(key)
            .is_some_and(|entry| entry.until > now)
    }

    /// Frees the key only for whoever still holds it — the local half of [`UNLOCK`], where the
    /// mutex does what the script does.
    fn remove_if_holds(&self, key: &str, token: &[u8]) -> bool {
        let now = Instant::now();
        let mut entries = self.entries();
        match entries.get(key) {
            Some(entry) if entry.until > now && entry.bytes == token => {
                entries.remove(key);
                true
            }
            _ => false,
        }
    }

    /// Drops what has run out. A key read once and never again would otherwise be kept for ever:
    /// the expiry on the read path answers correctly but frees nothing.
    fn spawn_sweeper(&self) {
        let store = self.clone();
        tokio::spawn(metrics::TASK_CACHE_SWEEPER.instrument(async move {
            let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                ticker.tick().await;
                let now = Instant::now();
                let mut entries = store.entries();
                entries.retain(|_, entry| entry.until > now);
                metrics::CACHE_LOCAL_ENTRIES.set(entries.len().saturating_into());
            }
        }));
    }

    /// Locks the map. It holds nothing that stays valid across a panic anyway, so a poisoned mutex
    /// is recovered rather than propagated — otherwise one stray panic would turn every later
    /// lookup into a panic of its own.
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Entry {
    fn new(bytes: Vec<u8>, ttl: Duration) -> Self {
        Self { bytes, until: Instant::now() + ttl }
    }
}

async fn connect_to_redis(url: Option<String>) -> Backend {
    let Some(url) = url else {
        tracing::warn!("no REDIS_HOST is set, the cached values are kept in this process");
        return Backend::Local(LocalStore::default())
    };
    match open(url).await {
        Ok(conn) => {
            tracing::info!(mode = %CacheMode::Redis, "the cached values are shared through Redis");
            Backend::Redis(conn)
        }
        Err(e) => {
            tracing::error!(error = %e, "couldn't connect to Redis, the cached values are kept in this process");
            Backend::Local(LocalStore::default())
        }
    }
}

async fn open(url: String) -> redis::RedisResult<ConnectionManager> {
    ConnectionManager::new(redis::Client::open(url)?).await
}

fn full_key(key: impl CacheKey) -> String {
    format!("{KEY_PREFIX}:{key}")
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_containers::SharedContainer;

    const A_MINUTE: Duration = Duration::from_secs(60);

    /// Stands in for the real keys, which live with the values they name rather than here.
    ///
    /// Keyed by the process as well as by the test, because the server is **reused between runs**:
    /// a key written with a lifetime of a minute is still there when the suite is run again, and
    /// the second run would read the first one's values. The per-test databases in
    /// [`crate::repo::test`] are named for the same reason.
    #[derive(Clone, Copy)]
    struct TestKey(u8);

    impl Display for TestKey {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "test:{}:{}", std::process::id(), self.0)
        }
    }

    impl CacheKey for TestKey {}

    static CONTAINER: SharedContainer = SharedContainer::new(
        "cache", "valkey/valkey", "9-alpine", 6379, &["Ready to accept connections"]);

    /// Every key carries the bot's own prefix. Nothing else would notice it going missing: a key
    /// written and read by the same wrong name still works, and the collision only shows up on a
    /// server somebody else is also using.
    #[test]
    fn a_key_names_the_bot_it_belongs_to() {
        let key = full_key(TestKey(0));
        assert!(key.starts_with("dgb:"), "the key must name the bot, got {key}");
        assert!(key.ends_with(&TestKey(0).to_string()), "and keep what the key type rendered");
    }

    #[tokio::test]
    async fn a_value_survives_a_round_trip() {
        on_both_backends(|cache| async move {
            let key = TestKey(1);

            cache.set_flag(key, true, A_MINUTE).await;
            assert_eq!(cache.get_flag(key).await, Some(true));

            cache.set_flag(key, false, A_MINUTE).await;
            assert_eq!(cache.get_flag(key).await, Some(false));
        }).await;
    }

    #[tokio::test]
    async fn a_structured_value_survives_a_round_trip() {
        on_both_backends(|cache| async move {
            let key = TestKey(2);
            let value = vec!["a".to_owned(), "b".to_owned()];

            cache.set_json(key, &value, A_MINUTE).await;
            assert_eq!(cache.get_json::<Vec<String>>(key).await, Some(value));
        }).await;
    }

    #[tokio::test]
    async fn an_absent_key_is_nothing_known() {
        on_both_backends(|cache| async move {
            assert_eq!(cache.get_flag(TestKey(3)).await, None);
        }).await;
    }

    #[tokio::test]
    async fn a_value_stops_being_known_once_its_time_is_up() {
        on_both_backends(|cache| async move {
            let key = TestKey(4);

            cache.set_flag(key, false, Duration::from_secs(1)).await;
            assert_eq!(cache.get_flag(key).await, Some(false));

            tokio::time::sleep(Duration::from_millis(1500)).await;
            assert_eq!(cache.get_flag(key).await, None);
        }).await;
    }

    #[tokio::test]
    async fn a_removed_value_is_gone_and_says_it_was_there() {
        on_both_backends(|cache| async move {
            let key = TestKey(5);

            assert!(!cache.remove(key).await);
            cache.set_flag(key, true, A_MINUTE).await;
            assert!(cache.remove(key).await);
            assert_eq!(cache.get_flag(key).await, None);
        }).await;
    }

    #[tokio::test]
    async fn a_lock_is_taken_once_and_freed_by_its_holder() {
        on_both_backends(|cache| async move {
            let key = TestKey(6);

            let token = cache.lock(key, A_MINUTE).await.expect("the key must be free");
            assert_eq!(cache.lock(key, A_MINUTE).await, None);

            assert!(cache.unlock(key, &token).await);
            assert!(cache.lock(key, A_MINUTE).await.is_some());
        }).await;
    }

    #[tokio::test]
    async fn a_lock_frees_itself_when_its_time_is_up() {
        on_both_backends(|cache| async move {
            let key = TestKey(7);

            assert!(cache.lock(key, Duration::from_secs(1)).await.is_some());
            tokio::time::sleep(Duration::from_millis(1500)).await;
            assert!(cache.lock(key, Duration::from_secs(1)).await.is_some());
        }).await;
    }

    /// A holder that outran its lifetime has already lost the lock. Freeing it then must leave the
    /// hold that replaced it alone, or a third caller gets in while the second is still working —
    /// which is the whole failure a lock is taken against.
    #[tokio::test]
    async fn a_stale_holder_cannot_free_somebody_else_s_lock() {
        on_both_backends(|cache| async move {
            let key = TestKey(14);

            let stale = cache.lock(key, Duration::from_secs(1)).await.expect("the key must be free");
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let fresh = cache.lock(key, A_MINUTE).await.expect("the lifetime must have run out");

            assert!(!cache.unlock(key, &stale).await, "the stale token must free nothing");
            assert_eq!(cache.lock(key, A_MINUTE).await, None, "the fresh hold must have survived");
            assert!(cache.unlock(key, &fresh).await);
        }).await;
    }

    #[tokio::test]
    async fn a_hit_never_reaches_the_loader() {
        on_both_backends(|cache| async move {
            let key = TestKey(8);
            // `read_through` takes a counter pair rather than naming one, and which pair is nothing
            // to this test: it asserts on what the loader was asked for, not on what was counted.
            // So it borrows a real one instead of making `metrics` expose its constructor.
            let counters = &metrics::CHAT_TOPICS;

            let loaded = cache.read_through(key, A_MINUTE, counters, || async { 42u8 }).await;
            assert_eq!(loaded, 42);

            let cached: u8 = cache.read_through(key, A_MINUTE, counters, || async {
                panic!("the loader must not be called for a value that is cached")
            }).await;
            assert_eq!(cached, 42);

            // Forgetting the value is what sends the next lookup back to the loader.
            cache.remove(key).await;
            let reloaded = cache.read_through(key, A_MINUTE, counters, || async { 43u8 }).await;
            assert_eq!(reloaded, 43);
        }).await;
    }

    #[tokio::test]
    async fn a_disabled_cache_knows_nothing_and_keeps_nothing() {
        let cache = disabled();

        cache.set_flag(TestKey(9), true, A_MINUTE).await;
        assert_eq!(cache.get_flag(TestKey(9)).await, None);

        // Nothing is held, so nothing is ever locked either.
        assert!(cache.lock(TestKey(10), A_MINUTE).await.is_some());
        assert!(cache.lock(TestKey(10), A_MINUTE).await.is_some());
    }

    /// The two tenants that can't work without somewhere to put a value get one anyway.
    #[tokio::test]
    async fn a_disabled_cache_still_lends_a_local_one() {
        let cache = disabled().or_local();

        cache.set_flag(TestKey(11), true, A_MINUTE).await;
        assert_eq!(cache.get_flag(TestKey(11)).await, Some(true));

        assert!(cache.lock(TestKey(12), A_MINUTE).await.is_some());
        assert_eq!(cache.lock(TestKey(12), A_MINUTE).await, None);
    }

    #[tokio::test]
    async fn an_unreachable_server_leaves_a_working_local_store() {
        // Port 1 is never a Redis; the bot must run anyway.
        let cache = Cache::connect(CacheConfig::redis("redis://localhost:1/".to_owned())).await;

        cache.set_flag(TestKey(13), true, A_MINUTE).await;
        assert_eq!(cache.get_flag(TestKey(13)).await, Some(true));
    }

    /// Runs the scenario against both live backends, so that every assertion above holds for
    /// either.
    ///
    /// The two run **at once** rather than one after the other: a scenario that waits for a
    /// lifetime to run out then waits once instead of twice, and the two stores share no state to
    /// race over — the local one is built here and belongs to this test, and the keys are a test's
    /// own. The whole test binary shares one server, which is why a test that needs isolation asks
    /// for a key of its own.
    async fn on_both_backends<F, Fut>(scenario: F)
    where
        F: Fn(Cache) -> Fut,
        Fut: Future<Output = ()>,
    {
        let port = CONTAINER.port().await;
        let redis = Cache::connect(CacheConfig::redis(format!("redis://localhost:{port}/"))).await;
        let local = Cache::connect(CacheConfig { mode: CacheMode::Local, url: None }).await;
        tokio::join!(scenario(redis), scenario(local));
    }

    fn disabled() -> Cache {
        Cache { backend: Backend::Disabled(LocalStore::default()) }
    }
}
