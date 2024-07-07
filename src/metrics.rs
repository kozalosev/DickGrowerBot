use axum::routing::get;
use axum_prometheus::PrometheusMetricLayer;
use once_cell::sync::Lazy;
use prometheus::{Encoder, Opts, TextEncoder};

/// Register additional metrics of our own structs by using this registry instance.
static REGISTRY: Lazy<Registry> = Lazy::new(|| Registry(prometheus::Registry::new()));

// Export special preconstructed counters for Teloxide's handlers.
pub static INLINE_COUNTER: Lazy<ComplexCommandCounters> = Lazy::new(|| {
    let opts = Opts::new("inline_usage_total", "count of inline queries processed by the bot");
    ComplexCommandCounters {
        invoked: Counter::new("inline (query)", opts.clone().const_label("state", "query")),
        finished: Counter::new("inline (chosen)", opts.const_label("state", "chosen")),
    }
});
pub static CMD_START_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_start", Opts::new("command_start_usage_total", "count of /start invocations"))
});
pub static CMD_HELP_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_help", Opts::new("command_help_usage_total", "count of /help invocations"))
});
pub static CMD_PRIVACY_COUNTER: Lazy<Counter> = Lazy::new(|| {
    Counter::new("command_privacy", Opts::new("command_privacy_usage_total", "count of /privacy invocations"))
});
pub static CMD_GROW_COUNTER: Lazy<BothModesCounters> = Lazy::new(|| {
    let opts = Opts::new("command_grow_usage_total", "count of /grow invocations");
    BothModesCounters {
        chat: Counter::new("command_grow (chat)", opts.clone().const_label("mode", "chat")),
        inline: Counter::new("command_grow (inline)", opts.const_label("mode", "inline")),
    }
});
pub static CMD_TOP_COUNTER: Lazy<BothModesCounters> = Lazy::new(|| {
    let opts = Opts::new("command_top_usage_total", "count of /top invocations");
    BothModesCounters {
        chat: Counter::new("command_top (chat)", opts.clone().const_label("mode", "chat")),
        inline: Counter::new("command_top (inline)", opts.const_label("mode", "inline")),
    }
});
pub static CMD_LOAN_COUNTER: Lazy<BothModesComplexCommandCounters> = Lazy::new(|| {
    let opts = Opts::new("command_loan_usage_total", "count of /loan invocations");
    let invoked_opts = opts.clone().const_label("state", "invoked");
    BothModesComplexCommandCounters {
        invoked: BothModesCounters {
            chat: Counter::new("command_loan (chat)", invoked_opts.clone().const_label("mode", "chat")),
            inline: Counter::new("command_loan (inline)", invoked_opts.const_label("mode", "inline"))
        },
        finished: Counter::new("command_loan (finished)", opts.const_label("state", "finished")
            .const_label("mode", "unknown"))
    }
});
pub static CMD_DOD_COUNTER: Lazy<BothModesCounters> = Lazy::new(|| {
    let opts = Opts::new("command_dick_of_day_usage_total", "count of /dick_of_day invocations");
    BothModesCounters {
        chat: Counter::new("command_dick_of_day (chat)", opts.clone().const_label("mode", "chat")),
        inline: Counter::new("command_dick_of_day (inline)", opts.const_label("mode", "inline")),
    }
});
pub static CMD_PVP_COUNTER: Lazy<BothModesCounters> = Lazy::new(|| {
    let opts = Opts::new("command_pvp_usage_total", "count of /pvp invocations");
    BothModesCounters {
        chat: Counter::new("command_pvp (chat)", opts.clone().const_label("mode", "chat")),
        inline: Counter::new("command_pvp (inline)", opts.const_label("mode", "inline")),
    }
});
pub static CMD_STATS: Lazy<BothModesCounters> = Lazy::new(|| {
    let opts = Opts::new("command_stats_usage_total", "count of /stats invocations");
    BothModesCounters {
        chat: Counter::new("command_stats (chat)", opts.clone().const_label("mode", "chat")),
        inline: Counter::new("command_stats (inline)", opts.const_label("mode", "inline")),
    }
});
pub static CMD_IMPORT: Lazy<ComplexCommandCounters> = Lazy::new(|| {
    let opts = Opts::new("command_import_usage_total", "count of /import invocations and successes");
    ComplexCommandCounters {
        invoked: Counter::new("command_import (invoked)", opts.clone().const_label("state", "invoked")),
        finished: Counter::new("command_import (finished)", opts.const_label("state", "finished")),
    }
});
pub static CMD_PROMO: Lazy<DeepLinkedCommandsCounters> = Lazy::new(|| {
    let opts = Opts::new("command_promo_usage_total", "count of /promo invocations and successes");
    DeepLinkedCommandsCounters {
        invoked_by_command: Counter::new("command_promo (invoked)", opts.clone().const_label("state", "invoked_by_command")),
        invoked_by_deeplink: Counter::new("deeplink_promo (invoked)", opts.clone().const_label("state", "invoked_by_deeplink")),
        finished: Counter::new("command_promo (finished)", opts.const_label("state", "finished")),
    }
});


pub fn init() -> axum::Router {
    let prometheus = REGISTRY
        .register(&INLINE_COUNTER.invoked)
        .register(&INLINE_COUNTER.finished)
        .register(&CMD_START_COUNTER)
        .register(&CMD_HELP_COUNTER)
        .register(&CMD_PRIVACY_COUNTER)
        .register(&CMD_GROW_COUNTER.chat)
        .register(&CMD_GROW_COUNTER.inline)
        .register(&CMD_TOP_COUNTER.chat)
        .register(&CMD_TOP_COUNTER.inline)
        .register(&CMD_LOAN_COUNTER.invoked.chat)
        .register(&CMD_LOAN_COUNTER.invoked.inline)
        .register(&CMD_LOAN_COUNTER.finished)
        .register(&CMD_DOD_COUNTER.chat)
        .register(&CMD_DOD_COUNTER.inline)
        .register(&CMD_PVP_COUNTER.chat)
        .register(&CMD_PVP_COUNTER.inline)
        .register(&CMD_STATS.chat)
        .register(&CMD_STATS.inline)
        .register(&CMD_IMPORT.invoked)
        .register(&CMD_IMPORT.finished)
        .register(&CMD_PROMO.invoked_by_command)
        .register(&CMD_PROMO.invoked_by_deeplink)
        .register(&CMD_PROMO.finished)
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
pub struct ComplexCommandCounters {
    invoked: Counter,
    finished: Counter,
}
pub struct BothModesCounters {
    pub chat: Counter,
    pub inline: Counter
}
pub struct BothModesComplexCommandCounters {
    pub invoked: BothModesCounters,
    pub finished: Counter
}
pub struct DeepLinkedCommandsCounters {
    pub invoked_by_command: Counter,
    pub invoked_by_deeplink: Counter,
    pub finished: Counter,
}
struct Registry(prometheus::Registry);

impl Counter {
    fn new(name: &str, opts: Opts) -> Counter {
        let c = prometheus::Counter::with_opts(opts)
            .unwrap_or_else(|e| panic!("unable to create {name} counter: {e}"));
        Counter { inner: c, name: name.to_string() }
    }

    pub fn inc(&self) {
        self.inner.inc()
    }
}

impl ComplexCommandCounters {
    pub fn invoked(&self) {
        self.invoked.inc()
    }

    pub fn finished(&self) {
        self.finished.inc()
    }
}

impl Registry {
    fn register(&self, counter: &Counter) -> &Self {
        self.0.register(Box::new(counter.inner.clone()))
            .unwrap_or_else(|e| panic!("unable to register the {} counter: {e}", counter.name));
        self
    }

    fn unwrap(&self) -> prometheus::Registry {
        self.0.clone()
    }
}
