use axum::routing::get;
use axum_prometheus::PrometheusMetricLayer;
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, TextEncoder};
use strum::IntoEnumIterator;
use teloxide::types::UpdateKind;
use tokio_metrics_collector::TaskMonitor;
use domain_types::traits::{ApproxInto, SaturatingInto};
use crate::config::MessageGroup;
use crate::domain::primitives::{Count, SupportedLanguage};
use crate::repo::{ChatMigrationOutcome, DeletionState, MessageKind, ScheduledDeletion};

/// Additional metrics of our own are registered into this registry by the constructors below.
static REGISTRY: Lazy<prometheus::Registry> = Lazy::new(prometheus::Registry::new);

// Export special preconstructed counters for Teloxide's handlers.
pub static INLINE_COUNTER: Lazy<ComplexCommandCounters> = Lazy::new(||
    ComplexCommandCounters::new("inline_usage_total", "count of inline queries processed by the bot", ["query", "chosen"]));
pub static CMD_START_COUNTER: Lazy<Counter> = Lazy::new(||
    Counter::new("command_start_usage_total", "count of /start invocations"));
pub static CMD_HELP_COUNTER: Lazy<Counter> = Lazy::new(||
    Counter::new("command_help_usage_total", "count of /help invocations"));
pub static CMD_PRIVACY_COUNTER: Lazy<Counter> = Lazy::new(||
    Counter::new("command_privacy_usage_total", "count of /privacy invocations"));
pub static CMD_SUPPORT_COUNTER: Lazy<Counter> = Lazy::new(||
    Counter::new("command_support_usage_total", "count of /support invocations"));
pub static BANNED_UPDATES_BLOCKED: Lazy<Counter> = Lazy::new(||
    Counter::new("banned_updates_blocked_total", "count of updates rejected because their sender is banned"));
pub static CMD_GROW_COUNTER: Lazy<BothModesCounters> = Lazy::new(||
    BothModesCounters::new("command_grow_usage_total", "count of /grow invocations"));
pub static CMD_TOP_COUNTER: Lazy<BothModesCounters> = Lazy::new(||
    BothModesCounters::new("command_top_usage_total", "count of /top invocations"));
pub static CMD_LOAN_COUNTER: Lazy<BothModesComplexCommandCounters> = Lazy::new(||
    BothModesComplexCommandCounters::new("command_loan_usage_total", "count of /loan invocations"));
pub static CMD_DOD_COUNTER: Lazy<BothModesCounters> = Lazy::new(||
    BothModesCounters::new("command_dick_of_day_usage_total", "count of /dick_of_day invocations"));
pub static CMD_PVP_COUNTER: Lazy<BothModesCounters> = Lazy::new(||
    BothModesCounters::new("command_pvp_usage_total", "count of /pvp invocations"));
pub static CMD_STATS: Lazy<BothModesCounters> = Lazy::new(||
    BothModesCounters::new("command_stats_usage_total", "count of /stats invocations"));
pub static CMD_SHRINKS: Lazy<Counter> = Lazy::new(||
    Counter::new("command_shrinks_usage_total", "count of the inline shrinks command invocations"));
pub static CMD_IMPORT: Lazy<ComplexCommandCounters> = Lazy::new(||
    ComplexCommandCounters::new("command_import_usage_total", "count of /import invocations and successes", ["invoked", "finished"]));
pub static CMD_PROMO: Lazy<DeepLinkedCommandsCounters> = Lazy::new(||
    DeepLinkedCommandsCounters::new("command_promo_usage_total", "count of /promo invocations and successes"));
pub static USER_SERVICE: Lazy<UserServiceCounters> = Lazy::new(||
    UserServiceCounters::new("user_service_get_total", "count of user-service get() resolutions, split by whether they were served from cache or sent over gRPC"));
pub static CMD_LANGUAGE: Lazy<LanguageCommandCounters> = Lazy::new(||
    LanguageCommandCounters::new("command_language_usage_total", "count of /language usage, by scope (personal/chat) and state (invoked when the command is used, finished when a language is actually changed)"));
pub static CHAT_LANGUAGE: Lazy<CacheSourceCounters> = Lazy::new(||
    CacheSourceCounters::new("chat_language_get_total", "count of chat-wide language resolutions, split by whether they were served from cache or read from the database"));
pub static CMD_TOPICS: Lazy<ComplexCommandCounters> = Lazy::new(||
    ComplexCommandCounters::new("command_topics_usage_total", "count of /topics invocations and changes of the setting", ["invoked", "finished"]));
pub static CHAT_TOPICS: Lazy<CacheSourceCounters> = Lazy::new(||
    CacheSourceCounters::new("chat_topics_get_total", "count of allowed-topics lookups, split by whether they were served from cache or read from the database"));
pub static BOT_ADMIN_LOOKUP: Lazy<BotAdminLookupCounters> = Lazy::new(||
    BotAdminLookupCounters::new("bot_admin_lookup_total", "count of lookups of the bot's right to delete messages in a chat, split by whether the cache knew the answer"));
pub static TOPIC_RESTRICTED: Lazy<Counter> = Lazy::new(||
    Counter::new("topic_restricted_total", "count of updates dropped because they came from a forum topic the chat doesn't let the bot work in"));
pub static USED_LANGUAGE: Lazy<SpokenLanguageCounter> = Lazy::new(||
    SpokenLanguageCounter::new("used_language_total", "count of updates by the sender's Telegram (client) language_code, region suffix kept — an anonymous proxy for the languages the audience speaks"));
pub static UPDATE_KIND: Lazy<UpdateKindCounter> = Lazy::new(||
    UpdateKindCounter::new("update_kind_total", "count of all updates received from Telegram, by update kind — the total is the bot's real update rate, and the breakdown shows how much of it is traffic no handler acts on"));
pub static SELF_DESTRUCTION: Lazy<SelfDestructionCounters> = Lazy::new(||
    SelfDestructionCounters::new("self_destruction_total", "count of the messages the self-destruction feature has finished with, by message group, kind (reply/command/inline) and the state the message ended in. These are the same four states as in self_destruction_finished, but counted once per message instead of shown as a current number"));
pub static SELF_DESTRUCTION_RETRIES: Lazy<SelfDestructionRetryCounters> = Lazy::new(||
    SelfDestructionRetryCounters::new("self_destruction_retries_total", "count of the attempts that failed but were worth trying again, by message group and kind. Kept out of self_destruction_total because a message can be retried many times and still end as removed"));
pub static SELF_DESTRUCTION_PENDING: Lazy<Gauge> = Lazy::new(||
    Gauge::new("self_destruction_pending", "number of messages waiting for their self-destruction; a number that only grows means the worker stopped draining the queue"));
pub static SELF_DESTRUCTION_BATCH_SIZE: Lazy<Histogram> = Lazy::new(||
    Histogram::new("self_destruction_batch_size",
        "how many messages one run of the worker took. Read it together with the duration of that run: short batches and long runs mean Telegram is slow, while batches that reach self_destruction_batch_limit mean the queue is full. The two need opposite changes to MSG_SELFDESTRUCT_POLL_SECS",
        &[0.0, 1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0]));
pub static SELF_DESTRUCTION_BATCH_LIMIT: Lazy<Gauge> = Lazy::new(||
    Gauge::new("self_destruction_batch_limit", "the value of MSG_SELFDESTRUCT_BATCH_SIZE, so that a graph can tell a full batch from a small one without knowing the setting"));
pub static SELF_DESTRUCTION_FINISHED: Lazy<SelfDestructionFinishedGauges> = Lazy::new(||
    SelfDestructionFinishedGauges::new("self_destruction_finished", "number of self-destructions that are done with and kept for inspection until the cleaning process runs, by state: removed (the bot took the message down), removed_before (someone else had already done it), expired (the message outlived Telegram's 48-hour limit while it waited) or failed (every attempt was refused)"));
pub static ANNOUNCEMENT_SHOWN: Lazy<AnnouncementCounter> = Lazy::new(||
    AnnouncementCounter::new("announcement_shown_total", "count of announcements shown at the end of the Dick of the Day message, split by the recipient's language"));
pub static CHAT_MIGRATION: Lazy<ChatMigrationCounter> = Lazy::new(||
    ChatMigrationCounter::new("chat_migration_total", "count of group to supergroup migrations the bot witnessed, by outcome: migrated when the chat came across whole, migrated_unanchored when it came across but left its inline half behind, untraceable when it wasn't known by its old id at all, conflict when both ids already had a row of their own"));
pub static DAILY_SHRINK: Lazy<DailyShrinkCounters> = Lazy::new(DailyShrinkCounters::new);
pub static TELEGRAM_REQUEST_ERRORS: Lazy<TelegramRequestErrorCounters> = Lazy::new(||
    TelegramRequestErrorCounters::new("telegram_request_errors_total", "count of failed requests to the Telegram Bot API, split by kind (connect/timeout/network/api/rate_limited/other). A spike of connect/timeout is the DPI-stalling signal; rate_limited means Telegram asked the bot to slow down, so the THROTTLE_* limits are set too high"));
pub static TELEGRAM_REQUEST_DURATION: Lazy<TelegramRequestDuration> = Lazy::new(||
    TelegramRequestDuration::new("telegram_request_duration_seconds",
        "how long a request to the Telegram Bot API took, by method and outcome — the failed ones are measured too, so that a timeout (the slowest case there is) is not missing from the histogram"));
pub static TELEGRAM_THROTTLE_QUEUE_FULL: Lazy<Counter> = Lazy::new(||
    Counter::new("telegram_throttle_queue_full_total", "how many times the throttle worker found its queue full, which means the schedulers are asking for more requests than Telegram's limits allow and the extra ones are waiting. Teloxide reports this at most once every 4 seconds, so this counts moments, not requests"));

pub static DB_POOL_CONNECTIONS_OPENED: Lazy<Counter> = Lazy::new(||
    Counter::new("db_pool_connections_opened_total", "count of new physical connections opened by the sqlx pool"));
pub static DB_POOL_IDLE_SECONDS: Lazy<Histogram> = Lazy::new(||
    Histogram::new("db_pool_connection_idle_seconds",
        "how long a connection sat idle in the pool before being handed out to a caller",
        &[0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0]));
pub static DB_POOL_CONNECTION_AGE_SECONDS: Lazy<Histogram> = Lazy::new(||
    Histogram::new("db_pool_connection_age_seconds",
        "how old a connection was (time since it was first opened) when it was returned to the pool",
        &[1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0]));

pub static TASK_WEBHOOK_SERVER: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("webhook_http_server"));
pub static TASK_POLLING_DISPATCHER: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("polling_dispatcher"));
pub static TASK_METRICS_SERVER: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("metrics_http_server"));
pub static TASK_DAILY_SHRINK: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("daily_shrink"));
pub static TASK_SELF_DESTRUCTION: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("self_destruction"));
pub static TASK_SELF_DESTRUCTION_CLEANING: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("self_destruction_cleaning"));
pub static TASK_USER_SERVICE_CACHE_CLEANUP: Lazy<TaskMonitor> = Lazy::new(|| task_monitor("user_service_cache_cleanup"));

pub fn init() -> (axum::Router, PrometheusMetricLayer<'static>) {
    force_registration();

    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
    let registry = REGISTRY.clone();
    let router = axum::Router::new()
        .route("/metrics", get(|| async move {
            let mut buffer = vec![];
            TextEncoder::new().encode(&registry.gather(), &mut buffer)
                .expect("unable to encode custom metrics");
            let custom_metrics = String::from_utf8(buffer)
                .expect("metrics buffer is not valid UTF-8");
            let auto_metrics = autometrics::prometheus_exporter::encode_to_string()
                .expect("unable to encode autometrics");

            metric_handle.render() + custom_metrics.as_str() + auto_metrics.as_str()
        }));
    (router, prometheus_layer)
}

/// Registers the live sqlx pool gauges (`db_pool_size`, `db_pool_idle_connections`), read directly
/// from `pool` at scrape time rather than polled on an interval. Call once `db_conn` is available.
pub fn register_db_pool_collector(pool: sqlx::Pool<sqlx::Postgres>) {
    REGISTRY.register(Box::new(DbPoolCollector::new(pool)))
        .unwrap_or_else(|e| panic!("unable to register the db pool collector: {e}"));
}

/// The counters are registered on the first dereference of their `Lazy` statics, so all of them
/// must be forced here to make even never incremented counters appear in the `/metrics` output.
///
/// The `TASK_*` monitors are the exception: each of them registers itself when its task is first
/// spawned, so `/metrics` lists only the tasks this process actually runs. The webhook server and
/// the polling dispatcher are never both in use, and the user-service cache cleanup runs only when
/// the integration is enabled.
fn force_registration() {
    Lazy::force(&INLINE_COUNTER);
    Lazy::force(&CMD_START_COUNTER);
    Lazy::force(&CMD_HELP_COUNTER);
    Lazy::force(&CMD_PRIVACY_COUNTER);
    Lazy::force(&CMD_SUPPORT_COUNTER);
    Lazy::force(&BANNED_UPDATES_BLOCKED);
    Lazy::force(&CMD_GROW_COUNTER);
    Lazy::force(&CMD_TOP_COUNTER);
    Lazy::force(&CMD_LOAN_COUNTER);
    Lazy::force(&CMD_DOD_COUNTER);
    Lazy::force(&CMD_PVP_COUNTER);
    Lazy::force(&CMD_STATS);
    Lazy::force(&CMD_SHRINKS);
    Lazy::force(&CMD_IMPORT);
    Lazy::force(&CMD_PROMO);
    Lazy::force(&USER_SERVICE);
    Lazy::force(&CMD_LANGUAGE);
    Lazy::force(&CHAT_LANGUAGE);
    Lazy::force(&CMD_TOPICS);
    Lazy::force(&CHAT_TOPICS);
    Lazy::force(&BOT_ADMIN_LOOKUP);
    Lazy::force(&TOPIC_RESTRICTED);
    Lazy::force(&USED_LANGUAGE);
    Lazy::force(&UPDATE_KIND);
    Lazy::force(&SELF_DESTRUCTION);
    Lazy::force(&SELF_DESTRUCTION_RETRIES);
    Lazy::force(&ANNOUNCEMENT_SHOWN);
    Lazy::force(&CHAT_MIGRATION);
    Lazy::force(&DAILY_SHRINK);
    Lazy::force(&TELEGRAM_REQUEST_ERRORS);
    Lazy::force(&TELEGRAM_REQUEST_DURATION);
    Lazy::force(&TELEGRAM_THROTTLE_QUEUE_FULL);

    Lazy::force(&DB_POOL_CONNECTIONS_OPENED);
    Lazy::force(&DB_POOL_IDLE_SECONDS);
    Lazy::force(&DB_POOL_CONNECTION_AGE_SECONDS);
}

static TASK_COLLECTOR_REGISTERED: Lazy<()> = Lazy::new(|| {
    REGISTRY.register(Box::new(tokio_metrics_collector::default_task_collector()))
        .unwrap_or_else(|e| panic!("unable to register the tokio task collector: {e}"));
});

fn task_monitor(label: &'static str) -> TaskMonitor {
    Lazy::force(&TASK_COLLECTOR_REGISTERED);
    let monitor = TaskMonitor::new();
    tokio_metrics_collector::default_task_collector()
        .add(label, monitor.clone())
        .unwrap_or_else(|e| panic!("unable to register the {label} task monitor: {e}"));
    monitor
}

/// Reads `pool.size()`/`pool.num_idle()` live at scrape time — no persistent metric state of its
/// own, so its `Desc`s are built once and its `MetricFamily`s are rebuilt on every `collect()`.
struct DbPoolCollector {
    pool: sqlx::Pool<sqlx::Postgres>,
    size_desc: prometheus::core::Desc,
    idle_desc: prometheus::core::Desc,
}

impl DbPoolCollector {
    fn new(pool: sqlx::Pool<sqlx::Postgres>) -> Self {
        let size_desc = prometheus::core::Desc::new(
            "db_pool_size".to_owned(),
            "current number of connections in the sqlx pool (in use + idle)".to_owned(),
            vec![], std::collections::HashMap::new())
            .expect("invalid db_pool_size desc");
        let idle_desc = prometheus::core::Desc::new(
            "db_pool_idle_connections".to_owned(),
            "current number of idle connections in the sqlx pool".to_owned(),
            vec![], std::collections::HashMap::new())
            .expect("invalid db_pool_idle_connections desc");
        Self { pool, size_desc, idle_desc }
    }
}

impl prometheus::core::Collector for DbPoolCollector {
    fn desc(&self) -> Vec<&prometheus::core::Desc> {
        vec![&self.size_desc, &self.idle_desc]
    }

    fn collect(&self) -> Vec<prometheus::proto::MetricFamily> {
        let size = prometheus::IntGauge::new("db_pool_size", "current number of connections in the sqlx pool (in use + idle)")
            .expect("invalid db_pool_size gauge");
        size.set(i64::from(self.pool.size()));
        let idle = prometheus::IntGauge::new("db_pool_idle_connections", "current number of idle connections in the sqlx pool")
            .expect("invalid db_pool_idle_connections gauge");
        idle.set(self.pool.num_idle().saturating_into());

        let mut out = size.collect();
        out.extend(idle.collect());
        out
    }
}

pub struct Counter(IntCounter);
pub struct Gauge(prometheus::IntGauge);
pub struct GaugeVec(prometheus::IntGaugeVec);
pub struct CounterVec(IntCounterVec);
pub struct Histogram(prometheus::Histogram);
pub struct HistogramVec(prometheus::HistogramVec);

pub struct ComplexCommandCounters {
    invoked: Counter,
    finished: Counter,
}
pub struct BothModesCounters {
    pub chat: Counter,
    pub inline: Counter,
}
pub struct BothModesComplexCommandCounters {
    pub invoked: BothModesCounters,
    pub finished: Counter,
}
pub struct DeepLinkedCommandsCounters {
    pub invoked_by_command: Counter,
    pub invoked_by_deeplink: Counter,
    pub finished: Counter,
}
pub struct UserServiceCounters {
    cache: Counter,
    sent: Counter,
}
pub struct LanguageCommandCounters {
    personal: CommandProgress,
    chat: CommandProgress,
}
pub struct CommandProgress {
    invoked: Counter,
    finished: Counter,
}
/// A pair of counters for a read-through cache: where each lookup was answered from.
pub struct CacheSourceCounters {
    cache: Counter,
    db: Counter,
}
/// Where a lookup of the bot's rights in a chat was answered from. A miss is not a failure — it
/// means the caller had to fall back, either to asking Telegram or to assuming it may go ahead.
pub struct BotAdminLookupCounters {
    cache: Counter,
    miss: Counter,
}

impl Counter {
    fn new(name: &str, help: &str) -> Self {
        let inner = IntCounter::with_opts(Opts::new(name, help))
            .unwrap_or_else(|e| panic!("unable to create the {name} counter: {e}"));
        REGISTRY.register(Box::new(inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {name} counter: {e}"));
        Self(inner)
    }

    pub fn inc(&self) {
        self.0.inc()
    }

    pub fn inc_by(&self, count: u64) {
        self.0.inc_by(count)
    }
}

impl Gauge {
    fn new(name: &str, help: &str) -> Self {
        let inner = prometheus::IntGauge::with_opts(Opts::new(name, help))
            .unwrap_or_else(|e| panic!("unable to create the {name} gauge: {e}"));
        REGISTRY.register(Box::new(inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {name} gauge: {e}"));
        Self(inner)
    }

    /// Takes any number that can reach an `i64`, so that the conversion Prometheus needs happens
    /// here rather than at every caller. A gauge is a `float64` on the wire whatever is put in it,
    /// and `IntGauge` holds an `i64` only to keep the arithmetic exact in the process.
    pub fn set(&self, value: impl SaturatingInto<i64>) {
        self.0.set(value.saturating_into())
    }
}

impl GaugeVec {
    fn new(name: &str, help: &str, labels: &[&str]) -> Self {
        let inner = prometheus::IntGaugeVec::new(Opts::new(name, help), labels)
            .unwrap_or_else(|e| panic!("unable to create the {name} gauge vec: {e}"));
        REGISTRY.register(Box::new(inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {name} gauge vec: {e}"));
        Self(inner)
    }

    /// Returns the child gauge identified by these label values, given in the same order as the
    /// labels passed to [`GaugeVec::new`].
    fn gauge(&self, label_values: &[&str]) -> Gauge {
        Gauge(self.0.with_label_values(label_values))
    }
}

impl Histogram {
    fn new(name: &str, help: &str, buckets: &[f64]) -> Self {
        let inner = prometheus::Histogram::with_opts(
            prometheus::HistogramOpts::new(name, help).buckets(buckets.to_vec()))
            .unwrap_or_else(|e| panic!("unable to create the {name} histogram: {e}"));
        REGISTRY.register(Box::new(inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {name} histogram: {e}"));
        Self(inner)
    }

    /// Takes any number that can reach an `f64`, so that a caller counting things doesn't convert
    /// them itself. A histogram's buckets are floats, whatever is measured into them.
    pub fn observe(&self, value: impl ApproxInto<f64>) {
        self.0.observe(value.approx_into())
    }
}

impl HistogramVec {
    fn new(name: &str, help: &str, labels: &[&str], buckets: &[f64]) -> Self {
        let inner = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(name, help).buckets(buckets.to_vec()), labels)
            .unwrap_or_else(|e| panic!("unable to create the {name} histogram vec: {e}"));
        REGISTRY.register(Box::new(inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {name} histogram vec: {e}"));
        Self(inner)
    }

    /// Records an observation on the child histogram identified by these label values, given in
    /// the same order as the labels passed to [`HistogramVec::new`].
    fn observe(&self, label_values: &[&str], value: f64) {
        self.0.with_label_values(label_values).observe(value)
    }
}

impl CounterVec {
    fn new(name: &str, help: &str, labels: &[&str]) -> Self {
        let inner = IntCounterVec::new(Opts::new(name, help), labels)
            .unwrap_or_else(|e| panic!("unable to create the {name} counter vec: {e}"));
        REGISTRY.register(Box::new(inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {name} counter vec: {e}"));
        Self(inner)
    }

    /// Returns the child counter identified by these label values, given in the
    /// same order as the labels passed to [`CounterVec::new`].
    fn counter(&self, label_values: &[&str]) -> Counter {
        Counter(self.0.with_label_values(label_values))
    }
}

impl ComplexCommandCounters {
    fn new(name: &str, help: &str, [invoked, finished]: [&str; 2]) -> Self {
        let vec = CounterVec::new(name, help, &["state"]);
        Self {
            invoked: vec.counter(&[invoked]),
            finished: vec.counter(&[finished]),
        }
    }

    pub fn invoked(&self) {
        self.invoked.inc()
    }

    pub fn finished(&self) {
        self.finished.inc()
    }
}

impl BothModesCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["mode"]);
        Self {
            chat: vec.counter(&["chat"]),
            inline: vec.counter(&["inline"]),
        }
    }
}

impl BothModesComplexCommandCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["state", "mode"]);
        Self {
            invoked: BothModesCounters {
                chat: vec.counter(&["invoked", "chat"]),
                inline: vec.counter(&["invoked", "inline"]),
            },
            finished: vec.counter(&["finished", "unknown"]),
        }
    }
}

impl DeepLinkedCommandsCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["state"]);
        Self {
            invoked_by_command: vec.counter(&["invoked_by_command"]),
            invoked_by_deeplink: vec.counter(&["invoked_by_deeplink"]),
            finished: vec.counter(&["finished"]),
        }
    }
}

impl UserServiceCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["source"]);
        Self {
            cache: vec.counter(&["cache"]),
            sent: vec.counter(&["sent"]),
        }
    }

    /// A `get()` resolution served from the local TTL cache.
    pub fn cache_hit(&self) {
        self.cache.inc()
    }

    /// A `get()` resolution that hit the network (an actual gRPC request).
    pub fn request_sent(&self) {
        self.sent.inc()
    }
}

impl LanguageCommandCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["scope", "state"]);
        let progress = |scope: &str| CommandProgress {
            invoked: vec.counter(&[scope, "invoked"]),
            finished: vec.counter(&[scope, "finished"]),
        };
        Self {
            personal: progress("personal"),
            chat: progress("chat"),
        }
    }

    /// Counters for the personal `/language` (a private chat).
    pub fn personal(&self) -> &CommandProgress {
        &self.personal
    }

    /// Counters for the chat-wide `/language` (a group chat).
    pub fn chat(&self) -> &CommandProgress {
        &self.chat
    }
}

impl CommandProgress {
    pub fn invoked(&self) {
        self.invoked.inc()
    }

    pub fn finished(&self) {
        self.finished.inc()
    }
}

impl CacheSourceCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["source"]);
        Self {
            cache: vec.counter(&["cache"]),
            db: vec.counter(&["db"]),
        }
    }

    /// A lookup served from the local TTL cache.
    pub fn cache_hit(&self) {
        self.cache.inc()
    }

    /// A lookup that hit the database.
    pub fn db_query(&self) {
        self.db.inc()
    }
}

impl BotAdminLookupCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["source"]);
        Self {
            cache: vec.counter(&["cache"]),
            miss: vec.counter(&["miss"]),
        }
    }

    /// The cache knew the answer.
    pub fn cache_hit(&self) {
        self.cache.inc()
    }

    /// The cache didn't, so the caller had to ask Telegram or go ahead without knowing.
    pub fn miss(&self) {
        self.miss.inc()
    }
}

/// Counts updates by the sender's Telegram `language_code` (see [`language_label`]).
pub struct SpokenLanguageCounter(CounterVec);

impl SpokenLanguageCounter {
    fn new(name: &str, help: &str) -> Self {
        Self(CounterVec::new(name, help, &["language"]))
    }

    /// Records one interaction from a person whose Telegram `language_code` is `code`
    /// (absent or unrecognized => `unknown`). Carries no user id — anonymous.
    pub fn record(&self, code: Option<&str>) {
        self.0.counter(&[&language_label(code)]).inc()
    }
}

/// Counts every update the bot receives, labeled by its [`UpdateKind`].
///
/// Unlike [`SpokenLanguageCounter`], this one also counts updates without a sender, so its sum is
/// the honest update rate. Compare it with the `command_*_usage_total` family: the difference is
/// the traffic no handler acts on (plain group messages, mostly).
pub struct UpdateKindCounter(CounterVec);

impl UpdateKindCounter {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["kind"]);
        for kind in UPDATE_KIND_LABELS {
            vec.counter(&[kind]);
        }
        Self(vec)
    }

    /// Record one received update of the given kind.
    pub fn record(&self, kind: &UpdateKind) {
        self.0.counter(&[update_kind_label(kind)]).inc()
    }
}

/// Every label [`update_kind_label`] can return.
const UPDATE_KIND_LABELS: [&str; 24] = [
    "message", "edited_message", "channel_post", "edited_channel_post",
    "business_connection", "business_message", "edited_business_message", "deleted_business_messages",
    "message_reaction", "message_reaction_count",
    "inline_query", "chosen_inline_result", "callback_query",
    "shipping_query", "pre_checkout_query", "purchased_paid_media",
    "poll", "poll_answer",
    "my_chat_member", "chat_member", "chat_join_request",
    "chat_boost", "removed_chat_boost",
    "error",
];

/// The metric label of an update kind: the Telegram field name it came in under. `error` means
/// teloxide couldn't parse the update — usually a Bot API addition we don't know yet.
fn update_kind_label(kind: &UpdateKind) -> &'static str {
    match kind {
        UpdateKind::Message(_) => "message",
        UpdateKind::EditedMessage(_) => "edited_message",
        UpdateKind::ChannelPost(_) => "channel_post",
        UpdateKind::EditedChannelPost(_) => "edited_channel_post",
        UpdateKind::BusinessConnection(_) => "business_connection",
        UpdateKind::BusinessMessage(_) => "business_message",
        UpdateKind::EditedBusinessMessage(_) => "edited_business_message",
        UpdateKind::DeletedBusinessMessages(_) => "deleted_business_messages",
        UpdateKind::MessageReaction(_) => "message_reaction",
        UpdateKind::MessageReactionCount(_) => "message_reaction_count",
        UpdateKind::InlineQuery(_) => "inline_query",
        UpdateKind::ChosenInlineResult(_) => "chosen_inline_result",
        UpdateKind::CallbackQuery(_) => "callback_query",
        UpdateKind::ShippingQuery(_) => "shipping_query",
        UpdateKind::PreCheckoutQuery(_) => "pre_checkout_query",
        UpdateKind::PurchasedPaidMedia(_) => "purchased_paid_media",
        UpdateKind::Poll(_) => "poll",
        UpdateKind::PollAnswer(_) => "poll_answer",
        UpdateKind::MyChatMember(_) => "my_chat_member",
        UpdateKind::ChatMember(_) => "chat_member",
        UpdateKind::ChatJoinRequest(_) => "chat_join_request",
        UpdateKind::ChatBoost(_) => "chat_boost",
        UpdateKind::RemovedChatBoost(_) => "removed_chat_boost",
        UpdateKind::Error(_) => "error",
    }
}

/// Counts the messages the self-destruction feature took care of, labeled by message group and
/// kind (the lowercase `MessageGroup` and `MessageKind` Display strings) and by outcome. An
/// inline message is never deleted, only replaced, so its `removed` means "the placeholder is up".
pub struct SelfDestructionCounters(CounterVec);

impl SelfDestructionCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["group", "kind", "outcome"]);
        for group in MessageGroup::iter() {
            for kind in MessageKind::iter() {
                for state in DeletionState::TERMINAL {
                    vec.counter(&[&group.to_string(), &kind.to_string(), &state.to_string()]);
                }
            }
        }
        Self(vec)
    }

    /// Records how one message ended. Called once per message, with the state its row is left in.
    /// So the sum of all outcomes is the number of messages the worker finished with, and the
    /// share of each outcome means something on its own.
    pub fn record(&self, group: MessageGroup, kind: MessageKind, state: DeletionState) {
        self.0.counter(&[&group.to_string(), &kind.to_string(), &state.to_string()]).inc()
    }
}

/// The attempts that failed but were worth making again. The outcome above says nothing about
/// them, so they are counted here.
pub struct SelfDestructionRetryCounters(CounterVec);

impl SelfDestructionRetryCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["group", "kind"]);
        for group in MessageGroup::iter() {
            for kind in MessageKind::iter() {
                vec.counter(&[&group.to_string(), &kind.to_string()]);
            }
        }
        Self(vec)
    }

    pub fn record(&self, group: MessageGroup, kind: MessageKind) {
        self.0.counter(&[&group.to_string(), &kind.to_string()]).inc()
    }
}

/// The rows the self-destruction is done with, by the state they were left in — the queue's own
/// history, kept until the cleaning process removes it.
pub struct SelfDestructionFinishedGauges(GaugeVec);

impl SelfDestructionFinishedGauges {
    fn new(name: &str, help: &str) -> Self {
        let vec = GaugeVec::new(name, help, &["state"]);
        for state in DeletionState::TERMINAL {
            vec.gauge(&[&state.to_string()]).set(0);
        }
        Self(vec)
    }

    /// Publishes one count per terminal state. Every state is written on each call, so one that has
    /// just been cleaned away goes back to zero instead of keeping its last value for ever.
    pub fn set_all(&self, counts: &[(DeletionState, Count<ScheduledDeletion>)]) {
        for state in DeletionState::TERMINAL {
            let count = counts.iter()
                .find(|(counted, _)| *counted == state)
                .map_or(Count::default(), |(_, count)| *count);
            self.0.gauge(&[&state.to_string()]).set(count);
        }
    }
}

/// Counts announcements actually shown at the end of the Dick of the Day message, labeled by the
/// recipient's resolved [`SupportedLanguage`] (with fallback, the audience's language — not
/// necessarily the language the borrowed text is written in).
pub struct AnnouncementCounter(CounterVec);

impl AnnouncementCounter {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["language"]);
        for lang in SupportedLanguage::ALL {
            vec.counter(&[&lang.to_string()]);
        }
        Self(vec)
    }

    /// Record one announcement shown to a recipient of the given language.
    pub fn record(&self, lang: SupportedLanguage) {
        self.0.counter(&[&lang.to_string()]).inc()
    }
}

/// Counts the group to supergroup migrations the bot witnessed, labeled by what became of the
/// chat. A migration is announced in both chats at once; only the announcement that lands in the
/// new supergroup is counted, so one migration is one sample.
pub struct ChatMigrationCounter(CounterVec);

impl ChatMigrationCounter {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["outcome"]);
        for outcome in ChatMigrationOutcome::iter() {
            vec.counter(&[&outcome.to_string()]);
        }
        Self(vec)
    }

    /// Record what became of one migrated chat.
    pub fn record(&self, outcome: ChatMigrationOutcome) {
        self.0.counter(&[&outcome.to_string()]).inc()
    }
}

/// Counters of the daily shrink job: the runs, the shrunk dicks, and the sent summaries.
/// Without them the job is visible in the logs only.
///
/// Alert on `daily_shrink_run_total` when it stops growing for more than 26 hours. That means the
/// scheduler died: it is a detached task and it stops for good if it can't compute the next
/// wake-up time (see `spawn_daily_shrink`). A day with nothing to shrink still counts as a run,
/// under the `empty` label, so a quiet day doesn't look like a dead scheduler.
pub struct DailyShrinkCounters {
    runs: CounterVec,
    victims: CounterVec,
    broadcasts: CounterVec,
}

impl DailyShrinkCounters {
    fn new() -> Self {
        let runs = CounterVec::new("daily_shrink_run_total",
            "count of daily shrink runs by outcome: succeeded when dicks were shrunk, empty when there was nothing to shrink today, failed when the shrinking statement itself errored", &["outcome"]);
        let victims = CounterVec::new("daily_shrink_victims_total",
            "count of dicks shrunk, by how their owners get to hear about it: broadcast when their chat can be messaged, inline_only when it can't and the shrinks command is the only way to see it, unreachable when the bot can't post to their chat anymore", &["delivery"]);
        let broadcasts = CounterVec::new("daily_shrink_broadcast_total",
            "count of per-chat shrink summaries by outcome: sent, failed when Telegram rejected the message, unreachable when it rejected it because the bot can't post to that chat at all (which marks the chat), and skipped for chats already marked and not tried again (one sample per chat, not per victim)", &["outcome"]);
        for outcome in ["succeeded", "empty", "failed"] {
            runs.counter(&[outcome]);
        }
        for delivery in ["broadcast", "inline_only", "unreachable"] {
            victims.counter(&[delivery]);
        }
        for outcome in ["sent", "failed", "unreachable", "skipped"] {
            broadcasts.counter(&[outcome]);
        }
        Self { runs, victims, broadcasts }
    }

    /// A run that shrank at least one dick.
    pub fn run_succeeded(&self) {
        self.runs.counter(&["succeeded"]).inc()
    }

    /// A run that found nothing to shrink. A normal day, not a problem.
    pub fn run_empty(&self) {
        self.runs.counter(&["empty"]).inc()
    }

    /// A run where the SQL statement failed. Nothing was shrunk and nothing was announced.
    pub fn run_failed(&self) {
        self.runs.counter(&["failed"]).inc()
    }

    /// `count` dicks shrunk in chats the bot can post to.
    pub fn victims_to_broadcast(&self, count: u64) {
        self.victims.counter(&["broadcast"]).inc_by(count)
    }

    /// `count` dicks shrunk in inline-only chats. Such chats get no summary message.
    pub fn victims_inline_only(&self, count: u64) {
        self.victims.counter(&["inline_only"]).inc_by(count)
    }

    /// `count` dicks shrunk in chats the bot can't post to anymore. They get no summary message
    /// either, until someone brings the bot back and runs a command there.
    pub fn victims_unreachable(&self, count: u64) {
        self.victims.counter(&["unreachable"]).inc_by(count)
    }

    /// One chat's summary delivered.
    pub fn broadcast_sent(&self) {
        self.broadcasts.counter(&["sent"]).inc()
    }

    /// One chat's summary was not delivered. Its members can still use the shrinks command.
    ///
    /// A transient failure: the same chat is tried again tomorrow. Watch this one — it used to
    /// carry the permanent failures too, which kept it growing forever.
    pub fn broadcast_failed(&self) {
        self.broadcasts.counter(&["failed"]).inc()
    }

    /// One chat's summary was rejected because the bot can't post there at all, so the chat has
    /// just been marked and won't be tried again until it comes back.
    pub fn broadcast_unreachable(&self) {
        self.broadcasts.counter(&["unreachable"]).inc()
    }

    /// `count` chats weren't even tried: they had already been marked unreachable.
    pub fn broadcast_skipped(&self, count: u64) {
        self.broadcasts.counter(&["skipped"]).inc_by(count)
    }
}

/// Counts failed requests to the Telegram Bot API, labeled by `kind`:
/// * `connect` (the connection couldn't be established — includes connect timeouts, the DPI/ТСПУ signal),
/// * `timeout` (a read/total timeout after connecting),
/// * `network` (other network/IO errors),
/// * `api` (a legitimate Telegram API error — the network was fine),
/// * `other` (anything else).
pub struct TelegramRequestErrorCounters(CounterVec);

impl TelegramRequestErrorCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["kind"]);
        for kind in ["connect", "timeout", "network", "api", "rate_limited", "other"] {
            vec.counter(&[kind]);
        }
        Self(vec)
    }

    /// Records one failed Telegram API request of the given `kind` (see the type docs).
    pub fn record(&self, kind: &str) {
        self.0.counter(&[kind]).inc()
    }
}

/// The latency of the Telegram Bot API, labeled by the method called and by how the request ended
/// (`ok`, or the same kinds [`TelegramRequestErrorCounters`] uses for the failures).
///
/// The buckets go up to well past the bot's own request timeout: the interesting samples are the
/// slow ones, and a request that is about to time out has to land in a bucket of its own rather
/// than in `+Inf` together with everything else.
pub struct TelegramRequestDuration(HistogramVec);

impl TelegramRequestDuration {
    fn new(name: &str, help: &str) -> Self {
        Self(HistogramVec::new(name, help, &["method", "outcome"],
            &[0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 17.0, 30.0]))
    }

    /// Records one finished Telegram API request. `method` is the payload name teloxide reports
    /// (e.g. `SendMessage`), a set fixed by the library, so the label stays bounded.
    pub fn record(&self, method: &str, outcome: &str, seconds: f64) {
        self.0.observe(&[method, outcome], seconds)
    }
}

/// The sender's Telegram `language_code`, kept whole (region suffix included) and lowercased
/// (`"zh-TW"` → `"zh-tw"`, `"EN-us"` → `"en-us"`); anything absent or not shaped like a language
/// tag becomes `"unknown"` (keeps the metric's label cardinality bounded).
fn language_label(code: Option<&str>) -> String {
    code.map(|c| c.trim().to_ascii_lowercase())
        .filter(|c| (2..=12).contains(&c.len())
            && c.starts_with(|ch: char| ch.is_ascii_lowercase())
            && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The `/metrics` body, as the endpoint would render our own registry. Shared with the tests of
/// the modules that record into it.
#[cfg(test)]
pub(crate) fn render_metrics() -> String {
    let mut buffer = vec![];
    TextEncoder::new().encode(&REGISTRY.gather(), &mut buffer)
        .expect("couldn't encode the metrics");
    String::from_utf8(buffer)
        .expect("the metrics buffer is not valid UTF-8")
}

#[cfg(test)]
mod tests {
    use once_cell::sync::Lazy;
    use strum::IntoEnumIterator;
    use crate::config::MessageGroup;
    use crate::domain::primitives::Count;
    use crate::repo::{ChatMigrationOutcome, DeletionState, MessageKind, ScheduledDeletion};
    use super::{CHAT_MIGRATION, DAILY_SHRINK, DB_POOL_CONNECTIONS_OPENED, DB_POOL_IDLE_SECONDS,
                DB_POOL_CONNECTION_AGE_SECONDS, SELF_DESTRUCTION, SELF_DESTRUCTION_BATCH_SIZE,
                SELF_DESTRUCTION_FINISHED, SELF_DESTRUCTION_RETRIES,
                SelfDestructionFinishedGauges, TASK_DAILY_SHRINK, language_label, render_metrics};

    /// The outcomes worth alerting on are the ones that should never be incremented, so they have
    /// to be exported at zero rather than spring into existence the first time a chat is lost.
    #[test]
    fn every_chat_migration_outcome_is_exported() {
        Lazy::force(&CHAT_MIGRATION);
        let rendered = render_metrics();

        for outcome in ChatMigrationOutcome::iter() {
            let series = format!("chat_migration_total{{outcome=\"{outcome}\"}}");
            assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
        }
    }

    /// The dashboard watches the failure rate here, and a ratio needs all of its series from the
    /// start. If the `failed` one appeared only with the first refused deletion, there would be
    /// nothing to compare it against before that.
    ///
    /// The outcomes are the terminal states, so this also keeps the two metrics on the same words:
    /// what `self_destruction_finished` shows as a current number, this one counts as a rate.
    #[test]
    fn every_self_destruction_series_is_exported() {
        Lazy::force(&SELF_DESTRUCTION);
        Lazy::force(&SELF_DESTRUCTION_RETRIES);
        let rendered = render_metrics();

        for group in MessageGroup::iter() {
            for kind in MessageKind::iter() {
                for state in DeletionState::TERMINAL {
                    let series = format!("self_destruction_total{{group=\"{group}\",kind=\"{kind}\",outcome=\"{state}\"}}");
                    assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
                }
                let series = format!("self_destruction_retries_total{{group=\"{group}\",kind=\"{kind}\"}}");
                assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
            }
        }
    }

    /// A retry is an attempt, not an ending, so it must not land in the same counter: a message
    /// retried twice and then removed is one `removed` plus two retries.
    #[test]
    fn a_retry_is_counted_apart_from_the_outcomes() {
        Lazy::force(&SELF_DESTRUCTION);
        Lazy::force(&SELF_DESTRUCTION_RETRIES);
        let group = MessageGroup::Notice;
        let kind = MessageKind::Reply;

        SELF_DESTRUCTION_RETRIES.record(group, kind);
        SELF_DESTRUCTION_RETRIES.record(group, kind);
        SELF_DESTRUCTION.record(group, kind, DeletionState::Removed);
        let rendered = render_metrics();

        assert!(rendered.contains("self_destruction_retries_total{group=\"notice\",kind=\"reply\"} 2"),
            "the retries are miscounted in:\n{rendered}");
        assert!(rendered.contains("self_destruction_total{group=\"notice\",kind=\"reply\",outcome=\"removed\"} 1"),
            "the outcome is miscounted in:\n{rendered}");
    }

    /// The buckets have to cover every batch size the setting allows, not only the default one.
    /// A run that lands in `+Inf` says nothing about how full the batch was.
    #[test]
    fn the_batch_size_is_bucketed_around_its_limit() {
        Lazy::force(&SELF_DESTRUCTION_BATCH_SIZE);
        let rendered = render_metrics();

        for le in ["0", "1", "5", "10", "25", "50", "100", "250", "500", "+Inf"] {
            let series = format!("self_destruction_batch_size_bucket{{le=\"{le}\"}}");
            assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
        }
    }

    /// The ratio between these states is what tells a healthy queue from a stuck one, so all of
    /// them have to be there from the start — including on a bot that has never failed a deletion.
    #[test]
    fn every_finished_self_destruction_state_is_exported() {
        Lazy::force(&SELF_DESTRUCTION_FINISHED);
        let rendered = render_metrics();

        for state in DeletionState::TERMINAL {
            let series = format!("self_destruction_finished{{state=\"{state}\"}}");
            assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
        }
    }

    /// A state that has just been cleaned away must fall back to zero rather than keep the last
    /// number it was given.
    #[test]
    fn a_cleaned_state_goes_back_to_zero() {
        let gauges = SelfDestructionFinishedGauges::new("test_self_destruction_finished",
            "the finished self-destructions of the test");

        gauges.set_all(&[(DeletionState::Failed, Count::<ScheduledDeletion>::new(3))]);
        assert_eq!(gauges.0.gauge(&["failed"]).0.get(), 3);
        assert_eq!(gauges.0.gauge(&["expired"]).0.get(), 0);

        gauges.set_all(&[]);
        assert_eq!(gauges.0.gauge(&["failed"]).0.get(), 0);
    }

    /// We alert on counters that stop growing, so every series must exist from the start.
    /// A `failed` series that appears only after the first failure gives no data at all
    /// before it, and an alert can't tell that from a healthy day.
    #[test]
    fn every_daily_shrink_series_is_exported() {
        Lazy::force(&DAILY_SHRINK);
        let rendered = render_metrics();

        let expected = [
            ("daily_shrink_run_total", "outcome", ["succeeded", "empty", "failed"].as_slice()),
            ("daily_shrink_victims_total", "delivery", ["broadcast", "inline_only", "unreachable"].as_slice()),
            ("daily_shrink_broadcast_total", "outcome", ["sent", "failed", "unreachable", "skipped"].as_slice()),
        ];
        for (metric, label, values) in expected {
            for value in values {
                let series = format!("{metric}{{{label}=\"{value}\"}}");
                assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
            }
        }
    }

    /// The sqlx pool hook metrics and the tokio task monitors are both registered lazily; this
    /// just confirms they actually make it into the `/metrics` output once forced, same as every
    /// other metric in this file.
    #[test]
    fn sqlx_and_tokio_task_metrics_are_exported() {
        Lazy::force(&DB_POOL_CONNECTIONS_OPENED);
        Lazy::force(&DB_POOL_IDLE_SECONDS);
        Lazy::force(&DB_POOL_CONNECTION_AGE_SECONDS);
        Lazy::force(&TASK_DAILY_SHRINK);
        let rendered = render_metrics();

        for series in [
            "db_pool_connections_opened_total",
            "db_pool_connection_idle_seconds_bucket",
            "db_pool_connection_age_seconds_bucket",
            "tokio_task_instrumented_count{task=\"daily_shrink\"}",
        ] {
            assert!(rendered.contains(series), "{series} is missing from:\n{rendered}");
        }
    }

    /// A task monitor belongs to `/metrics` only when its task runs in this process. The dashboards
    /// would otherwise show flat lines for the half of the pair that is never used: the polling
    /// dispatcher under a webhook, or the webhook server while polling.
    #[test]
    fn a_task_that_was_never_spawned_is_not_exported() {
        Lazy::force(&TASK_DAILY_SHRINK);
        let rendered = render_metrics();

        assert!(rendered.contains("task=\"daily_shrink\""), "the collector is not registered at all:\n{rendered}");
        assert!(!rendered.contains("task=\"polling_dispatcher\""), "an unused task is exported:\n{rendered}");
    }

    #[test]
    fn language_label_normalization() {
        assert_eq!(language_label(Some("ru")), "ru");
        assert_eq!(language_label(Some("zh-TW")), "zh-tw");
        assert_eq!(language_label(Some("EN-us")), "en-us");
        assert_eq!(language_label(Some("pt-BR")), "pt-br");
        assert_eq!(language_label(Some("es-419")), "es-419");
        assert_eq!(language_label(Some("  ")), "unknown");
        assert_eq!(language_label(Some("")), "unknown");
        assert_eq!(language_label(Some("x")), "unknown");
        assert_eq!(language_label(Some("123")), "unknown");
        assert_eq!(language_label(None), "unknown");
    }
}
