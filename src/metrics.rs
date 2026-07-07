use std::collections::HashMap;
use std::sync::Mutex;
use axum::routing::get;
use axum_prometheus::PrometheusMetricLayer;
use linkme::distributed_slice;
use once_cell::sync::Lazy;
use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, TextEncoder};

/// Declare, register and increment a counter for a handler function.
/// See the documentation in the `metrics-macro` crate for the details.
pub use metrics_macro::{command_handler, inline_chosen_handler, inline_handler};

/// Additional metrics of our own are registered into this registry by the constructors below.
static REGISTRY: Lazy<prometheus::Registry> = Lazy::new(prometheus::Registry::new);

/// Descriptors of the counters declared by the [`handler`] attribute macro all over the codebase.
/// The linker collects them into this slice; [`init`] registers all of them at startup.
#[distributed_slice]
pub static COUNTERS: [CounterDesc];

/// Counter families created on demand from [`CounterDesc`]riptors, deduplicated by the metric
/// name: several call sites may declare (and increment) different series of the same family.
static DECLARED_FAMILIES: Lazy<Mutex<HashMap<&'static str, Family>>> = Lazy::new(Default::default);

// Export special preconstructed counters for Teloxide's handlers.
pub static CMD_START_COUNTER: Lazy<Counter> = Lazy::new(||
    Counter::new("command_start_usage_total", "count of /start invocations"));
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
    ComplexCommandCounters::new("command_import_usage_total", "count of /import invocations and successes"));
pub static CMD_PROMO: Lazy<DeepLinkedCommandsCounters> = Lazy::new(||
    DeepLinkedCommandsCounters::new("command_promo_usage_total", "count of /promo invocations and successes"));

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

/// The counters are registered on the first dereference of their `Lazy` statics (or, for those
/// declared by the [`handler`] attribute macro, on the first increment), so all of them must be
/// forced here to make even never incremented counters appear in the `/metrics` output.
fn force_registration() {
    for desc in COUNTERS {
        desc.counter();
    }
    Lazy::force(&CMD_START_COUNTER);
    Lazy::force(&CMD_GROW_COUNTER);
    Lazy::force(&CMD_TOP_COUNTER);
    Lazy::force(&CMD_LOAN_COUNTER);
    Lazy::force(&CMD_DOD_COUNTER);
    Lazy::force(&CMD_PVP_COUNTER);
    Lazy::force(&CMD_STATS);
    Lazy::force(&CMD_IMPORT);
    Lazy::force(&CMD_PROMO);
}

/// Increments the counter described by a [`CounterDesc`], creating and registering its metric
/// family if this is the first use. Invoked by the code the [`handler`] attribute macro generates.
pub fn inc(desc: &CounterDesc) {
    desc.counter().inc()
}

/// A description of a counter declared by the [`handler`] attribute macro: the metric family
/// (name, help, label names) plus the label values identifying one series within that family.
pub struct CounterDesc {
    pub name: &'static str,
    pub help: &'static str,
    pub label_names: &'static [&'static str],
    pub label_values: &'static [&'static str],
}

impl CounterDesc {
    /// Returns the counter for this descriptor, creating and registering the family on first use.
    fn counter(&self) -> Counter {
        let mut families = DECLARED_FAMILIES.lock().expect("the declared families map is poisoned");
        let family = families.entry(self.name).or_insert_with(|| {
            if self.label_names.is_empty() {
                Family::Plain(Counter::new(self.name, self.help))
            } else {
                Family::Labeled(CounterVec::new(self.name, self.help, self.label_names))
            }
        });
        match family {
            Family::Plain(counter) => counter.clone(),
            Family::Labeled(vec) => vec.counter(self.label_values),
        }
    }
}

/// A registered metric family a [`CounterDesc`] resolves into.
enum Family {
    Plain(Counter),
    Labeled(CounterVec),
}

#[derive(Clone)]
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
    fn new(name: &str, help: &str) -> Self {
        let vec = CounterVec::new(name, help, &["state"]);
        Self {
            invoked: vec.counter(&["invoked"]),
            finished: vec.counter(&["finished"]),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_counters_are_registered_and_incremented() {
        static PLAIN: CounterDesc = CounterDesc {
            name: "test_plain_total",
            help: "a counter without labels",
            label_names: &[],
            label_values: &[],
        };
        static LABELED: CounterDesc = CounterDesc {
            name: "test_labeled_total",
            help: "a counter with a label",
            label_names: &["mode"],
            label_values: &["chat"],
        };

        inc(&PLAIN);
        inc(&LABELED);
        inc(&LABELED);

        let families = REGISTRY.gather();

        let plain = families.iter().find(|f| f.get_name() == PLAIN.name)
            .expect("the plain counter must be registered");
        assert_eq!(plain.get_metric()[0].get_counter().get_value() as u64, 1);

        let labeled = families.iter().find(|f| f.get_name() == LABELED.name)
            .expect("the labeled counter must be registered");
        let metric = &labeled.get_metric()[0];
        assert_eq!(metric.get_counter().get_value() as u64, 2);
        assert_eq!(metric.get_label()[0].get_name(), "mode");
        assert_eq!(metric.get_label()[0].get_value(), "chat");
    }

    /// Ensures the linker actually collects the descriptors declared by the `handler` attribute
    /// macro all over the codebase — the whole approach hinges on this distributed slice.
    #[test]
    fn handler_attribute_populates_the_distributed_slice() {
        let names: Vec<&str> = COUNTERS.iter().map(|desc| desc.name).collect();
        assert!(names.contains(&"command_help_usage_total"), "got: {names:?}");
        assert!(names.contains(&"command_privacy_usage_total"), "got: {names:?}");
        assert!(names.contains(&"inline_usage_total"), "got: {names:?}");
    }
}
