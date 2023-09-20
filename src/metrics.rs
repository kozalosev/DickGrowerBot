use axum::routing::get;
use axum_prometheus::PrometheusMetricLayer;
use once_cell::sync::Lazy;
use prometheus::{Encoder, Opts, TextEncoder};

/// Register additional metrics of our own structs by using this registry instance.
static REGISTRY: Lazy<Registry> = Lazy::new(|| Registry(prometheus::Registry::new()));

// Export special preconstructed counters for Teloxide's handlers.
pub static INLINE_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("inline", Opts::new("inline_usage_total", "count of inline queries processed by the bot"))
});
pub static INLINE_CHOSEN_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("inline_chosen", Opts::new("inline_chosen_total", "count of inline results chosen by the users"))
});
pub static MESSAGE_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("message", Opts::new("message_usage_total", "count of messages processed by the bot"))
});
pub static CMD_START_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_start", Opts::new("command_start_usage_total", "count of /start invocations"))
});
pub static CMD_HELP_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_help", Opts::new("command_help_usage_total", "count of /help invocations"))
});
pub static CMD_GROW_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_grow", Opts::new("command_grow_usage_total", "count of /grow invocations"))
});
pub static CMD_TOP_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_top", Opts::new("command_top_usage_total", "count of /top invocations"))
});

pub fn init() -> axum::Router {
    let prometheus = REGISTRY
        .register(&*INLINE_COUNTER)
        .register(&*INLINE_CHOSEN_COUNTER)
        .register(&*MESSAGE_COUNTER)
        .register(&*CMD_START_COUNTER)
        .register(&*CMD_HELP_COUNTER)
        .register(&*CMD_GROW_COUNTER)
        .register(&*CMD_TOP_COUNTER)
        .unwrap();

    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
    axum::Router::new()
        .route("/metrics", get(|| async move {
            let mut buffer = vec![];
            let metrics = prometheus.gather();
            TextEncoder::new().encode(&metrics, &mut buffer).unwrap();
            let custom_metrics = String::from_utf8(buffer).unwrap();

            metric_handle.render() + custom_metrics.as_str()
        }))
        .layer(prometheus_layer)
}

pub struct Counter {
    inner: prometheus::Counter,
    name: String
}
struct Registry(prometheus::Registry);

impl Counter {
    fn new(name: &str, opts: Opts) -> Counter {
        let c = prometheus::Counter::with_opts(opts)
            .expect(format!("unable to create {name} counter").as_str());
        Counter { inner: c, name: name.to_string() }
    }

    pub fn inc(&self) {
        self.inner.inc()
    }
}

impl Registry {
    fn register(&self, counter: &Counter) -> &Self {
        self.0.register(Box::new(counter.inner.clone()))
            .expect(format!("unable to register the {} counter", counter.name).as_str());
        self
    }

    fn unwrap(&self) -> prometheus::Registry {
        self.0.clone()
    }
}
