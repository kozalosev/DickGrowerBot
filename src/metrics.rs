use axum::routing::get;
use axum_prometheus::PrometheusMetricLayer;
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, TextEncoder};

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
pub static CMD_IMPORT: Lazy<ComplexCommandCounters> = Lazy::new(||
    ComplexCommandCounters::new("command_import_usage_total", "count of /import invocations and successes", ["invoked", "finished"]));
pub static CMD_PROMO: Lazy<DeepLinkedCommandsCounters> = Lazy::new(||
    DeepLinkedCommandsCounters::new("command_promo_usage_total", "count of /promo invocations and successes"));
pub static USER_SERVICE: Lazy<UserServiceCounters> = Lazy::new(||
    UserServiceCounters::new("user_service_get_total", "count of user-service get() resolutions, split by whether they were served from cache or sent over gRPC"));
pub static CMD_LANGUAGE: Lazy<LanguageCommandCounters> = Lazy::new(||
    LanguageCommandCounters::new("command_language_usage_total", "count of /language invocations, split by whether they targeted the personal or the chat-wide language"));
pub static CHAT_LANGUAGE: Lazy<ChatLanguageCounters> = Lazy::new(||
    ChatLanguageCounters::new("chat_language_get_total", "count of chat-wide language resolutions, split by whether they were served from cache or read from the database"));

pub fn init() -> axum::Router {
    force_registration();

    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
    let registry = REGISTRY.clone();
    axum::Router::new()
        .route("/metrics", get(|| async move {
            let mut buffer = vec![];
            TextEncoder::new().encode(&registry.gather(), &mut buffer)
                .expect("unable to encode custom metrics");
            let custom_metrics = String::from_utf8(buffer)
                .expect("metrics buffer is not valid UTF-8");

            metric_handle.render() + custom_metrics.as_str()
        }))
        .layer(prometheus_layer)
}

/// The counters are registered on the first dereference of their `Lazy` statics, so all of them
/// must be forced here to make even never incremented counters appear in the `/metrics` output.
fn force_registration() {
    Lazy::force(&INLINE_COUNTER);
    Lazy::force(&CMD_START_COUNTER);
    Lazy::force(&CMD_HELP_COUNTER);
    Lazy::force(&CMD_PRIVACY_COUNTER);
    Lazy::force(&CMD_GROW_COUNTER);
    Lazy::force(&CMD_TOP_COUNTER);
    Lazy::force(&CMD_LOAN_COUNTER);
    Lazy::force(&CMD_DOD_COUNTER);
    Lazy::force(&CMD_PVP_COUNTER);
    Lazy::force(&CMD_STATS);
    Lazy::force(&CMD_IMPORT);
    Lazy::force(&CMD_PROMO);
    Lazy::force(&USER_SERVICE);
    Lazy::force(&CMD_LANGUAGE);
    Lazy::force(&CHAT_LANGUAGE);
}

pub struct Counter(IntCounter);
pub struct CounterVec(IntCounterVec);

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
    personal: Counter,
    chat: Counter,
}
pub struct ChatLanguageCounters {
    cache: Counter,
    db: Counter,
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
        let vec = CounterVec::new(name, help, &["scope"]);
        Self {
            personal: vec.counter(&["personal"]),
            chat: vec.counter(&["chat"]),
        }
    }

    /// A `/language` invocation that targeted the caller's personal language (a private chat).
    pub fn personal(&self) {
        self.personal.inc()
    }

    /// A `/language` invocation that targeted the chat-wide language (a group chat).
    pub fn chat(&self) {
        self.chat.inc()
    }
}

impl ChatLanguageCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["source"]);
        Self {
            cache: vec.counter(&["cache"]),
            db: vec.counter(&["db"]),
        }
    }

    /// A chat-language resolution served from the local TTL cache.
    pub fn cache_hit(&self) {
        self.cache.inc()
    }

    /// A chat-language resolution that hit the database.
    pub fn db_query(&self) {
        self.db.inc()
    }
}
