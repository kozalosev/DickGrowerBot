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
//!
//! [`Backend::Redis`] rides out a live outage the same way [`Cache::or_local`] rides out being
//! disabled: [`RedisState::fallback`] is a local store it always carries, and once
//! [`RedisHealth::is_degraded`] trips, every call routes there instead of to Redis — see
//! [`Backend::route`]. Only the map itself is built up front; its sweeper waits for that first
//! trip, so a Redis that never fails never pays for one. [`sync_fallback_to_redis`] hands whatever
//! is still there back to Redis the moment it answers again, before the routing switches back.

use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
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
const SWEEP_INTERVAL: Duration = Duration::from_mins(1);

/// How often a degraded [`Backend::Redis`] is probed to see whether it can be used again.
///
/// This is what actually finds out — nothing else does, since a degraded call never touches Redis
/// at all. A constant for the same reason as [`SWEEP_INTERVAL`]: it only decides how promptly the
/// bot notices Redis is back, not whether anything works while it waits.
const REDIS_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(5);

/// How long Redis must have been answering before [`RedisState::fallback_sweeper`] is stopped —
/// not for good, just until the next outage restarts it, see [`Backend::route`].
///
/// A constant for the same reason as [`SWEEP_INTERVAL`]: correctness doesn't wait on this. The
/// fallback is already unused the moment Redis recovers — `Backend::route` never sends a call
/// there again until the next failure — so all this decides is how long an idle sweeper keeps
/// ticking after that before it is worth reclaiming.
const REDIS_FALLBACK_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

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
        match &backend {
            Backend::Disabled(store) | Backend::Local(store) => { store.spawn_sweeper(); }
            Backend::Redis(state) => spawn_redis_health_check(state.clone(), REDIS_FALLBACK_IDLE_TIMEOUT),
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
        match self.backend.route() {
            Route::Disabled => false,
            Route::Local(store) => store.remove(&key),
            Route::Redis(state) => state.health.record(state.conn.clone().del::<_, usize>(&key).await)
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
        let taken = match self.backend.route() {
            Route::Disabled => true,
            Route::Local(store) => store.insert_if_absent(&key, token.0.clone().into_bytes(), ttl),
            Route::Redis(state) => {
                let options = redis::SetOptions::default()
                    .conditional_set(redis::ExistenceCheck::NX)
                    .with_expiration(redis::SetExpiry::EX(ttl.as_secs()));
                state.health.record(state.conn.clone().set_options::<_, _, Option<String>>(&key, &token.0, options).await)
                    .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't take a lock, letting the caller through"))
                    // This one call must not stop the bot working, so it counts as free — the same
                    // fallback a miss gets everywhere else here. The next one isn't reached at all —
                    // see `Backend::route` — and the lock is a real one again, taken against
                    // `RedisState::fallback` instead.
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
        match self.backend.route() {
            Route::Disabled => false,
            Route::Local(store) => store.remove_if_holds(&key, token.0.as_bytes()),
            Route::Redis(state) => state.health.record(UNLOCK.key(&key).arg(&token.0)
                .invoke_async::<usize>(&mut state.conn.clone()).await)
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
        match self.backend.route() {
            Route::Disabled => None,
            Route::Local(store) => store.get(key),
            Route::Redis(state) => state.health.record(state.conn.clone().get::<_, Option<Vec<u8>>>(key).await)
                .inspect_err(|e| tracing::warn!(error = %e, key, "couldn't read a value from the cache"))
                .ok()
                .flatten(),
        }
    }

    async fn set_raw(&self, key: &str, value: Vec<u8>, ttl: Duration) {
        match self.backend.route() {
            Route::Disabled => {}
            Route::Local(store) => store.insert(key, value, ttl),
            Route::Redis(state) => state.health.record(state.conn.clone().set_ex::<_, _, ()>(key, value, ttl.as_secs()).await)
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
    Redis(Arc<RedisState>),
}

impl Backend {
    /// Resolves to the store a call should actually use. A healthy `Redis` routes to itself;
    /// everything else — a degraded `Redis` included — routes to a local store, and only
    /// `Disabled` refuses to hold anything at all. Kept as one place rather than a fourth arm
    /// repeated in every method, since a degraded call is handled by the exact code a real
    /// `Backend::Local` already has.
    fn route(&self) -> Route<'_> {
        match self {
            Backend::Disabled(_) => Route::Disabled,
            Backend::Local(store) => Route::Local(store),
            Backend::Redis(state) if state.health.is_degraded() => {
                let mut sweeper = lock(&state.fallback_sweeper);
                if sweeper.is_none() {
                    *sweeper = Some(state.fallback.spawn_sweeper());
                }
                Route::Local(&state.fallback)
            }
            Backend::Redis(state) => Route::Redis(state),
        }
    }
}

enum Route<'a> {
    Disabled,
    Local(&'a LocalStore),
    Redis(&'a RedisState),
}

/// A live Redis connection, and everything needed to ride out a stretch where it stops answering.
struct RedisState {
    conn: ConnectionManager,
    /// Where the values go while [`RedisHealth::is_degraded`] — this instance's own store, exactly
    /// what [`Backend::Local`] would use. Built once, up front, so falling into it costs nothing to
    /// make.
    fallback: LocalStore,
    /// Started lazily by [`Backend::route`] on the first call that actually needs
    /// [`fallback`](Self::fallback), and stopped by [`spawn_redis_health_check`] once Redis has
    /// been answering for [`REDIS_FALLBACK_IDLE_TIMEOUT`] — `None` the rest of the time, so a
    /// Redis that never fails, or one that recovered a while ago, never pays for it.
    fallback_sweeper: Mutex<Option<tokio::task::JoinHandle<()>>>,
    health: RedisHealth,
}

/// Whether Redis answered the last time it was asked, kept apart from the connection itself so the
/// state machine can be exercised without one.
///
/// Trips on the **first** failure rather than waiting for a few in a row: `ConnectionManager`
/// already retries internally with its own growing backoff before a call returns at all, so
/// tolerating several failures here would mean paying that backoff several times over — the exact
/// wait this exists to avoid. Nothing is lost by tripping early: `RedisState::fallback` is a
/// correct store for the one instance of the bot that ever runs, not a degraded one, so there is no
/// downside to using it a little sooner.
struct RedisHealth {
    /// Cleared only by [`spawn_redis_health_check`] once a probe succeeds again.
    degraded: AtomicBool,
    /// When [`degraded`](Self::degraded) last changed, either way — what
    /// [`spawn_redis_health_check`] measures a healthy stretch against.
    changed_at: Mutex<Instant>,
}

impl RedisHealth {
    fn new() -> Self {
        Self { degraded: AtomicBool::new(false), changed_at: Mutex::new(Instant::now()) }
    }

    fn is_degraded(&self) -> bool {
        self.degraded.load(Ordering::Relaxed)
    }

    /// How long Redis has been answering since it last stopped, or `None` while it still isn't.
    fn healthy_for(&self) -> Option<Duration> {
        (!self.is_degraded()).then(|| lock(&self.changed_at).elapsed())
    }

    /// Folds one call's outcome into [`degraded`](Self::degraded), logged only on the calls that
    /// change the mode, not on every one that confirms it. Also keeps
    /// [`metrics::CACHE_FALLBACK_ACTIVE`] following the most recent call, so a live outage shows up
    /// there too and not only one that was never reached at startup.
    fn record<T, E>(&self, result: Result<T, E>) -> Result<T, E> {
        metrics::CACHE_FALLBACK_ACTIVE.set(result.is_err().into());
        match &result {
            Ok(_) => if self.degraded.swap(false, Ordering::Relaxed) {
                *lock(&self.changed_at) = Instant::now();
                tracing::info!("Redis answered again, sharing the cached values through it once more");
            },
            Err(_) => if !self.degraded.swap(true, Ordering::Relaxed) {
                *lock(&self.changed_at) = Instant::now();
                tracing::warn!("couldn't reach Redis, keeping the cached values in this process until it answers again");
            },
        }
        result
    }
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
    ///
    /// Returns the handle so [`RedisState::fallback_sweeper`] can abort it once it stops earning
    /// its keep; the other two callers just let theirs run for the life of the process.
    fn spawn_sweeper(&self) -> tokio::task::JoinHandle<()> {
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
        }))
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        lock(&self.0)
    }

    /// Takes every entry still worth keeping, each with however much of its lifetime is left —
    /// what [`sync_fallback_to_redis`] needs to hand them on rather than let them expire unread.
    /// Already-expired entries are dropped rather than returned, the same as every other read here.
    fn drain_live(&self) -> Vec<DrainedEntry> {
        let now = Instant::now();
        self.entries()
            .drain()
            .filter_map(|(key, entry)| (entry.until > now)
                .then(|| DrainedEntry { key, bytes: entry.bytes, ttl: entry.until - now }))
            .collect()
    }
}

/// One value taken out of a [`LocalStore`] by [`LocalStore::drain_live`], with what's left of its
/// lifetime — a named field for each rather than a tuple, since `.1` said nothing about which was
/// the key and which the value at the one call site this has, let alone at a second one.
struct DrainedEntry {
    key: String,
    bytes: Vec<u8>,
    ttl: Duration,
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
            Backend::Redis(Arc::new(RedisState {
                conn,
                fallback: LocalStore::default(),
                fallback_sweeper: Mutex::new(None),
                health: RedisHealth::new(),
            }))
        }
        Err(e) => {
            tracing::error!(error = %e, "couldn't connect to Redis, the cached values are kept in this process");
            Backend::Local(LocalStore::default())
        }
    }
}

/// Retries Redis on a timer while [`RedisHealth::is_degraded`], rather than on the data path: a
/// degraded call is routed away from Redis entirely so it isn't paying its timeout (see
/// [`Backend::route`]), and something still has to notice when it is safe to use again.
///
/// `idle_timeout` is [`REDIS_FALLBACK_IDLE_TIMEOUT`] in production — a parameter rather than the
/// constant read directly, so a test can ask for the teardown without waiting minutes for it.
fn spawn_redis_health_check(state: Arc<RedisState>, idle_timeout: Duration) {
    tokio::spawn(metrics::TASK_CACHE_REDIS_HEALTH_CHECK.instrument(async move {
        let mut ticker = tokio::time::interval(REDIS_HEALTH_CHECK_INTERVAL);
        loop {
            ticker.tick().await;

            match state.health.healthy_for() {
                None => {
                    let result = state.conn.clone().ping::<String>().await;
                    let recovered = state.health.record(result)
                        .inspect_err(|e| tracing::debug!(error = %e, "Redis is still not answering"))
                        .is_ok();
                    if recovered {
                        sync_fallback_to_redis(&state).await;
                    }
                }
                Some(healthy_for) if healthy_for >= idle_timeout => {
                    // Locked before the re-check, not after: a call that fails between
                    // `healthy_for` above and here must find its sweeper still running, or
                    // `Backend::route` — which only starts one when it finds `None` — would leave
                    // a degraded backend with none at all until the *next* failure restarts one.
                    let mut sweeper = lock(&state.fallback_sweeper);
                    if !state.health.is_degraded()
                        && let Some(handle) = sweeper.take() {
                        handle.abort();
                        tracing::debug!("Redis has been reachable for a while, stopping the fallback's sweeper");
                    }
                }
                Some(_) => {}
            }
        }
    }));
}

/// Copies whatever [`RedisState::fallback`] still holds into Redis the moment it answers again, so
/// a lock or a dialogue that outlived the outage doesn't vanish the instant [`Backend::route`]
/// stops reading from the fallback. Each entry keeps whatever is left of its original lifetime
/// rather than starting a fresh one, and is drained from the fallback either way — a failure here
/// is logged and the whole batch given up on, the same as every other failure in this file, not
/// retried.
async fn sync_fallback_to_redis(state: &RedisState) {
    let entries = state.fallback.drain_live();
    if entries.is_empty() {
        return;
    }
    let count = entries.len();

    let mut pipeline = redis::pipe();
    for entry in entries {
        pipeline.set_ex(entry.key, entry.bytes, entry.ttl.as_secs().max(1)).ignore();
    }
    if let Err(e) = pipeline.query_async::<()>(&mut state.conn.clone()).await {
        tracing::warn!(error = %e, count, "couldn't sync the fallback into Redis");
    }
}

async fn open(url: String) -> redis::RedisResult<ConnectionManager> {
    ConnectionManager::new(redis::Client::open(url)?).await
}

fn full_key(key: impl CacheKey) -> String {
    format!("{KEY_PREFIX}:{key}")
}

/// Locks a mutex, recovering it if poisoned rather than propagating the panic: nothing behind one
/// of these stays invalid across a panic, so poisoning would only turn a later, unrelated call into
/// a panic of its own.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::test_containers::SharedContainer;

    const A_MINUTE: Duration = Duration::from_mins(1);

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

    /// `RedisHealth::record` must hand back exactly what it was given — every Redis arm relies on
    /// chaining `.inspect_err`/`.ok`/`.is_ok_and` onto its result unchanged.
    #[test]
    fn redis_health_record_passes_the_result_through() {
        let health = RedisHealth::new();
        assert_eq!(health.record::<u8, &str>(Ok(5)), Ok(5));
        assert_eq!(health.record::<u8, &str>(Err("boom")), Err("boom"));
    }

    #[test]
    fn redis_health_falls_back_on_the_first_failure_and_recovers_on_one_success() {
        let health = RedisHealth::new();
        assert!(!health.is_degraded());

        let _ = health.record::<(), ()>(Err(()));
        assert!(health.is_degraded(), "one failure must be enough to trip it");

        let _ = health.record::<(), ()>(Ok(()));
        assert!(!health.is_degraded(), "one success must undo it");
    }

    /// After that first failure, a call stops trying Redis altogether and starts working again
    /// against [`RedisState::fallback`] — this is what makes the real fallback more than a fast
    /// miss: a lock or a dialogue kept during the outage actually means what it says.
    /// [`spawn_redis_health_check`] is what notices Redis is back, lets calls use it again, and
    /// syncs what the outage wrote into the fallback into Redis before `Backend::route` stops
    /// reading from it — see [`sync_fallback_to_redis`].
    ///
    /// Paused rather than stopped: Docker Desktop hands a *stopped* container a new host port on
    /// its next start, which would leave `cache` pointed at a port nothing listens on any more and
    /// make recovery untestable for a reason that has nothing to do with this file. A paused
    /// container keeps its port and freezes instead, which the client sees as a server that stopped
    /// answering — the same thing a live outage looks like. A container of its own, not
    /// [`CONTAINER`]: pausing the one every other test in this file shares would break them.
    #[tokio::test]
    async fn a_sustained_outage_falls_back_to_a_working_store_and_recovers_on_its_own() {
        let (container, cache) = a_cache_with_its_own_container().await;
        let Backend::Redis(state) = &cache.backend else { panic!("must have connected to Redis") };
        let key = TestKey(16);

        container.pause().await.expect("couldn't pause the container");
        let result = tokio::time::timeout(Duration::from_secs(5), cache.get_flag(key)).await
            .expect("a call against a dead server must fail on its own, not hang until this timeout");
        assert_eq!(result, None, "a call against a dead server is a miss, like every other failure here");
        assert!(state.health.is_degraded(), "the failure must have tripped the fallback");

        let started = Instant::now();
        cache.set_flag(key, true, A_MINUTE).await;
        assert_eq!(cache.get_flag(key).await, Some(true),
            "a degraded backend must actually work against its fallback store, not just miss");
        assert!(started.elapsed() < Duration::from_millis(200),
            "and must not even try the dead server while degraded");

        container.unpause().await.expect("couldn't unpause the container");
        tokio::time::timeout(REDIS_HEALTH_CHECK_INTERVAL * 6, async {
            while state.health.is_degraded() {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }).await.expect("the health check must notice Redis answers again");

        assert_eq!(cache.get_flag(key).await, Some(true),
            "what the outage wrote must have been synced into Redis, not left behind in the fallback");
        assert!(state.fallback.entries().is_empty(), "and the fallback must have been drained of it");
    }

    /// Two more properties of a healthy `Redis`, each seeded directly rather than paid for with a
    /// real outage — that end-to-end path is the test above. They share one container, since
    /// neither ever pauses or stops it the way that one does.
    #[tokio::test]
    async fn a_healthy_redis_syncs_a_seeded_fallback_and_stops_its_idle_sweeper() {
        let (_container, cache) = a_cache_with_its_own_container().await;
        let Backend::Redis(state) = &cache.backend else { panic!("must have connected to Redis") };

        // sync_fallback_to_redis: two entries, not one — sync sends every entry as a single
        // pipeline, and this is what proves more than one command in it actually lands.
        let (first, second) = (TestKey(18), TestKey(19));
        state.fallback.insert(&full_key(first), TRUE.to_vec(), A_MINUTE);
        state.fallback.insert(&full_key(second), FALSE.to_vec(), A_MINUTE);
        sync_fallback_to_redis(state).await;
        assert_eq!(cache.get_flag(first).await, Some(true), "the first synced value must be readable");
        assert_eq!(cache.get_flag(second).await, Some(false), "and the second, from the same pipeline");
        assert!(state.fallback.entries().is_empty(), "and both gone from the fallback");

        // The idle-sweeper teardown: seed a running sweeper directly rather than paying for
        // another outage to start one, and ask spawn_redis_health_check for a short idle_timeout.
        *lock(&state.fallback_sweeper) = Some(state.fallback.spawn_sweeper());
        assert!(!state.health.is_degraded(), "healthy since connect, with nothing having failed");
        spawn_redis_health_check(state.clone(), Duration::from_millis(50));
        tokio::time::timeout(REDIS_HEALTH_CHECK_INTERVAL * 2, async {
            while lock(&state.fallback_sweeper).is_some() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("an idle sweeper must be stopped once Redis has been healthy long enough");
    }

    /// A Valkey container of its own, not [`CONTAINER`]: stopping the one every other test in this
    /// file shares would break them.
    async fn a_cache_with_its_own_container() -> (testcontainers::ContainerAsync<testcontainers::GenericImage>, Cache) {
        use testcontainers::GenericImage;
        use testcontainers::core::{IntoContainerPort, WaitFor};
        use testcontainers::runners::AsyncRunner;

        let container = GenericImage::new("valkey/valkey", "9-alpine")
            .with_exposed_port(6379.tcp())
            .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
            .start().await.expect("couldn't start a Valkey container of its own");
        let port = container.get_host_port_ipv4(6379).await.expect("couldn't fetch its port");
        let cache = Cache::connect(CacheConfig::redis(format!("redis://localhost:{port}/"))).await;
        (container, cache)
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
        let local = Cache::connect(CacheConfig::without_redis(CacheMode::Local)).await;
        tokio::join!(scenario(redis), scenario(local));
    }

    fn disabled() -> Cache {
        Cache { backend: Backend::Disabled(LocalStore::default()) }
    }
}
