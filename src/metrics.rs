use axum::routing::get;
use axum_prometheus::PrometheusMetricLayer;
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, TextEncoder};
use crate::domain::primitives::SupportedLanguage;

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
    LanguageCommandCounters::new("command_language_usage_total", "count of /language usage, by scope (personal/chat) and state (invoked when the command is used, finished when a language is actually changed)"));
pub static CHAT_LANGUAGE: Lazy<ChatLanguageCounters> = Lazy::new(||
    ChatLanguageCounters::new("chat_language_get_total", "count of chat-wide language resolutions, split by whether they were served from cache or read from the database"));
pub static USED_LANGUAGE: Lazy<SpokenLanguageCounter> = Lazy::new(||
    SpokenLanguageCounter::new("used_language_total", "count of updates by the sender's Telegram (client) language_code, region suffix kept — an anonymous proxy for the languages the audience speaks"));
pub static SELF_DESTRUCTION: Lazy<SelfDestructionCounters> = Lazy::new(||
    SelfDestructionCounters::new("self_destruction_total", "count of the bot's own messages removed by the self-destruction feature, split by message group and outcome (deleted/failed)"));
pub static ANNOUNCEMENT_SHOWN: Lazy<AnnouncementCounter> = Lazy::new(||
    AnnouncementCounter::new("announcement_shown_total", "count of announcements shown at the end of the Dick of the Day message, split by the recipient's language"));

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
            let auto_metrics = autometrics::prometheus_exporter::encode_to_string()
                .expect("unable to encode autometrics");

            metric_handle.render() + custom_metrics.as_str() + auto_metrics.as_str()
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
    Lazy::force(&USED_LANGUAGE);
    Lazy::force(&SELF_DESTRUCTION);
    Lazy::force(&ANNOUNCEMENT_SHOWN);
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
    personal: CommandProgress,
    chat: CommandProgress,
}
pub struct CommandProgress {
    invoked: Counter,
    finished: Counter,
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

/// Counts the bot's own messages removed by the self-destruction feature, labeled by
/// message group (the lowercase `MessageGroup` Display string) and outcome.
pub struct SelfDestructionCounters(CounterVec);

impl SelfDestructionCounters {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["group", "outcome"]);
        for group in ["notice", "report"] {
            for outcome in ["deleted", "failed"] {
                vec.counter(&[group, outcome]);
            }
        }
        Self(vec)
    }

    /// Record the result of one self-destruction deletion for `group` (its lowercase
    /// `MessageGroup` Display string): `deleted` when the message was removed, otherwise failed.
    pub fn record(&self, group: &str, deleted: bool) {
        let outcome = if deleted { "deleted" } else { "failed" };
        self.0.counter(&[group, outcome]).inc()
    }
}

/// Counts announcements actually shown at the end of the Dick of the Day message, labeled by the
/// recipient's resolved [`SupportedLanguage`] (with fallback, the audience's language — not
/// necessarily the language the borrowed text is written in).
pub struct AnnouncementCounter(CounterVec);

impl AnnouncementCounter {
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["language"]);
        // Pre-create every language's series so they all appear in /metrics even at zero.
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

#[cfg(test)]
mod tests {
    use super::language_label;

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
