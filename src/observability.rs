use std::collections::HashMap;
use std::error::Error;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::{OpenTelemetryTracingBridge, TracingSpanAttributes};
use opentelemetry_sdk::Resource;
use opentelemetry_otlp::{Compression, LogExporter, SpanExporter, WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor, Sampler, SdkTracerProvider};
use tracing_subscriber::filter::{filter_fn, FilterExt};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use domain_types::literal;
use crate::config::{env_duration, secs};
use crate::config::get_env_value_or_default;
use crate::domain::primitives::{Coefficient, Ratio};

const SERVICE_NAME: &str = env!("CARGO_PKG_NAME");

const TRACES_ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const LOGS_ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT";
const SPAN_FILTER_VAR: &str = "OTEL_SPAN_FILTER";
const SAMPLE_RATIO_VAR: &str = "OTEL_TRACES_SAMPLE_RATIO";

/// How much of what is written ever becomes a span, before the sampler is even asked.
///
/// `sqlx` is off because it logs every finished query, and a query is the leaf of nearly every span
/// here: at `trace` one broadcast tick alone turns a few hundred sends into tens of thousands of
/// events. The bot's own per-item spans sit at `debug` for the same reason, so raising this to
/// `debug` is what brings them back while a scheduler is being looked into.
const DEFAULT_SPAN_FILTER: &str = "info,h2=off,hyper=off,tower=off,teloxide=info,reqwest=info,sqlx=off";

/// Records of these crates are never exported. The exporter logs while it sends, and those records
/// would be sent again — a loop that feeds itself. They still go to the console.
const NEVER_EXPORTED_TARGETS: [&str; 5] = ["opentelemetry", "hyper", "h2", "tower", "reqwest"];

/// The providers to shut down before the process exits, so the last batch reaches the collector.
pub struct Telemetry {
    tracer_provider: Option<SdkTracerProvider>,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Telemetry {
    pub fn shutdown(&self) -> Result<(), Box<dyn Error>> {
        if let Some(tracer_provider) = &self.tracer_provider {
            tracer_provider.shutdown()?;
        }
        if let Some(logger_provider) = &self.logger_provider {
            logger_provider.shutdown()?;
        }
        Ok(())
    }
}

/// Initializes the tracing subscriber: the console output, and the OpenTelemetry export of spans
/// and log records when the infrastructure is configured.
///
/// The console layer is always on. It is the fallback: `docker logs` and journald keep working, and
/// it is what remains when the collector can't be reached.
///
/// The `log::*` records of the libraries (teloxide, sqlx, reqwest) are bridged into the tracing
/// pipeline by `tracing_subscriber`'s built-in `tracing-log` feature, installed inside `try_init`,
/// so they end up in the same output as our own events.
///
/// Configuration via environment variables:
/// - `RUST_LOG`: verbosity of the console and of the exported records;
/// - `OTEL_EXPORTER_OTLP_ENDPOINT`: where the spans go, over gRPC. Unset => spans are not exported;
/// - `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`: where the log records go, over HTTP (the full URL, e.g.
///   `http://victoria-logs:9428/insert/opentelemetry/v1/logs`). Unset => the console only;
/// - `OTEL_SPAN_FILTER`: verbosity of the spans, which `RUST_LOG` does not reach — see
///   [`DEFAULT_SPAN_FILTER`];
/// - `OTEL_TRACES_SAMPLE_RATIO`: the share of traces kept — see [`sampler`];
/// - `OTEL_BSP_QUEUE_SIZE`, `OTEL_BSP_BATCH_SIZE`, `OTEL_BSP_DELAY`: how the spans are batched.
///
/// The spans have a verbosity of their own because the two answer different questions: `RUST_LOG`
/// is what a human reads, and the filter is how much of the program's shape is worth keeping.
///
/// The two signals need two endpoints because they usually live in different places and speak
/// different protocols. Trace and span ids are attached to the exported records by the SDK itself,
/// which is why the console lines carry no ids: without the infrastructure there is nothing to
/// match them against anyway.
///
/// An exported record also carries the fields of every span it was written inside, from the root
/// down, so a value a log message keeps out of its text is searchable all the same. The nearer
/// span wins where two of them name the same field.
pub fn init_tracing() -> Result<Telemetry, Box<dyn Error>> {
    let tracer_provider = build_tracer_provider()?;
    let spans_exported = tracer_provider.is_some();
    if let Some(provider) = &tracer_provider {
        global::set_tracer_provider(provider.clone());
    }
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    // The span layer has a verbosity of its own: RUST_LOG governs what a human reads, and this
    // governs how much of it becomes a span at all.
    let telemetry_layer = tracer_provider.as_ref().map(|provider| {
        let filter = EnvFilter::new(get_env_value_or_default(
            SPAN_FILTER_VAR, DEFAULT_SPAN_FILTER.to_owned()));
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(SERVICE_NAME))
            // A span's thread and source location are five attributes apiece that nothing here
            // queries by, and tracked inactivity is two clock reads per enter and exit.
            .with_threads(false)
            .with_location(false)
            .with_tracked_inactivity(false)
            .with_filter(filter)
    });

    let logger_provider = endpoint(LOGS_ENDPOINT_VAR)
        .map(build_logger_provider)
        .transpose()?;
    let logs_layer = logger_provider.as_ref().map(|provider| {
        let filter = EnvFilter::from_default_env()
            .and(filter_fn(|metadata| !NEVER_EXPORTED_TARGETS.iter()
                .any(|target| metadata.target().starts_with(target))));
        build_logs_bridge(provider).with_filter(filter)
    });
    let logs_exported = logs_layer.is_some();

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_filter(EnvFilter::from_default_env());

    tracing_subscriber::registry()
        .with(telemetry_layer)
        .with(logs_layer)
        .with(fmt_layer)
        .try_init()?;

    // Only now is there a subscriber to write them to.
    tracing::info!(service_name = SERVICE_NAME, "tracing initialized");
    if spans_exported {
        tracing::info!(variable = TRACES_ENDPOINT_VAR, "the spans are exported");
    } else {
        tracing::warn!(variable = TRACES_ENDPOINT_VAR, "the variable is not set, the spans are not exported");
    }
    if logs_exported {
        tracing::info!(variable = LOGS_ENDPOINT_VAR, "the log records are exported");
    } else {
        tracing::warn!(variable = LOGS_ENDPOINT_VAR, "the variable is not set, the logs go to the console only");
    }
    Ok(Telemetry { tracer_provider, logger_provider })
}

/// Installs a process-wide panic hook that routes every panic through the tracing pipeline
/// instead of Rust's default, which writes straight to stderr — bypassing the console formatting,
/// the OTLP log export, and `panics_total`. Must be called after [`init_tracing`], since it logs
/// through the subscriber that installs.
///
/// A hook covers every panic in the process, including one inside a fire-and-forget background
/// task ([`crate::scheduler`] spawns each worker without keeping its `JoinHandle`), which is the
/// case a per-call-site `catch_unwind` would miss unless it were added everywhere by hand. Such a
/// panic otherwise kills that task for good with nothing to say why — only the task's own
/// heartbeat going stale, minutes to hours later, in the way that was diagnosed the hard way
/// before this existed.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map_or_else(|| "unknown".to_owned(), ToString::to_string);
        let payload = info.payload();
        let message = payload.downcast_ref::<&str>().copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("<non-string panic payload>");
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_owned();
        let backtrace = std::backtrace::Backtrace::capture();

        crate::metrics::PANICS_TOTAL.inc(&[&location]);
        tracing::error!(%location, message, %thread, %backtrace, "a panic was caught");
    }));
}

/// The provider, or nothing when there is nowhere to send spans. Nothing means the layer is left
/// off the subscriber entirely, so a bot without a collector doesn't pay to build spans that only
/// get dropped.
fn build_tracer_provider() -> Result<Option<SdkTracerProvider>, Box<dyn Error>> {
    let Some(endpoint) = endpoint(TRACES_ENDPOINT_VAR) else {
        return Ok(None);
    };

    let otlp_exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_compression(Compression::Gzip)
        .build()?;
    // The stock queue of 2048 is smaller than a single broadcast tick, so the spans of a busy
    // minute are dropped before the exporter thread ever wakes up to send them.
    let batch = BatchConfigBuilder::default()
        .with_max_queue_size(get_env_value_or_default("OTEL_BSP_QUEUE_SIZE", 8192))
        .with_max_export_batch_size(get_env_value_or_default("OTEL_BSP_BATCH_SIZE", 2048))
        .with_scheduled_delay(env_duration!("OTEL_BSP_DELAY", or = secs(2), at_least = secs(1)))
        .build();
    let processor = BatchSpanProcessor::builder(otlp_exporter)
        .with_batch_config(batch)
        .build();
    Ok(Some(SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_sampler(sampler())
        .with_resource(resource())
        .build()))
}

/// How large a share of the traces is kept.
///
/// Parent-based, so a decision taken at the root holds for every span beneath it and a trace never
/// arrives with holes in it. That also means the unit being sampled is a whole trace: for the
/// schedulers, whose root span is one tick of the loop, it is the tick that is kept or dropped.
///
/// The default keeps everything, so setting nothing changes nothing.
fn sampler() -> Sampler {
    let ratio = get_env_value_or_default(SAMPLE_RATIO_VAR, literal!(Ratio = 1.0));
    Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio.as_f64())))
}

/// The endpoint is always passed explicitly: left to itself, the exporter would fall back to
/// `OTEL_EXPORTER_OTLP_ENDPOINT` and send the log records to the tracing backend.
///
/// `VL-Stream-Fields` is read by VictoriaLogs and tells it which fields identify a log stream. Left
/// alone, it takes every resource attribute, and the SDK puts the name of the event — which is the
/// file and the line it was written at — among them. That would make a stream per log statement,
/// and a new set of them after every edit of the source.
///
/// `VL-Msg-Field` names the fields to take the message from, the first non-empty one winning. Not
/// every event has a message: sqlx logs a finished query as fields only, with the statement in
/// `summary`, and such a record would be stored as "missing _msg field" otherwise.
///
/// Other backends ignore both headers.
fn build_logger_provider(endpoint: String) -> Result<SdkLoggerProvider, Box<dyn Error>> {
    let headers = HashMap::from([
        ("VL-Stream-Fields".to_owned(), "service.name".to_owned()),
        ("VL-Msg-Field".to_owned(), "_msg,summary".to_owned()),
    ]);
    let otlp_exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .with_headers(headers)
        .build()?;
    Ok(SdkLoggerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource())
        .build())
}

/// Turns the tracing events into OTLP log records, each carrying the fields of the spans it was
/// written inside.
///
/// Every field is taken rather than a named few: what a span carries is already chosen by hand at
/// each `#[tracing::instrument]`, and a list here would be a second one to keep in step with it.
fn build_logs_bridge(
    provider: &SdkLoggerProvider,
) -> OpenTelemetryTracingBridge<SdkLoggerProvider, SdkLogger> {
    OpenTelemetryTracingBridge::builder(provider)
        .with_tracing_span_attributes(TracingSpanAttributes::all())
        .build()
}

fn endpoint(variable: &str) -> Option<String> {
    std::env::var(variable).ok().filter(|value| !value.is_empty())
}

/// Identifies this service in both signals, so a trace and a log record can be told apart from
/// those of the other bots sharing the same collector.
fn resource() -> Resource {
    Resource::builder()
        .with_service_name(SERVICE_NAME.to_owned())
        .build()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use opentelemetry::trace::{TraceContextExt, TracerProvider};
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;
    use super::*;
    use crate::test_containers::SharedContainer;

    const VICTORIA_LOGS_PORT: u16 = 9428;
    const INGESTION_PATH: &str = "/insert/opentelemetry/v1/logs";

    static CONTAINER: SharedContainer = SharedContainer::new(
        "victoria-logs", "victoriametrics/victoria-logs", "latest", VICTORIA_LOGS_PORT, &[])
        .with_settle_millis(500);

    /// The record must carry the ids of the span it was written in, put there by the SDK, and the
    /// fields of that span. Everything here is the real pipeline: the tracing bridge, the OTLP/HTTP
    /// exporter, and the same log database the server runs. The fields must survive as fields, not
    /// as text inside the message.
    ///
    /// `chat_id` is set on the span and never on an event, so nothing but the span can be carrying
    /// it when the assertion finds it.
    #[tokio::test]
    async fn an_exported_record_carries_the_trace_id_and_the_fields() {
        let base_url = victoria_logs().await;
        let logger_provider = build_logger_provider(format!("{base_url}{INGESTION_PATH}"))
            .expect("couldn't build the logger provider");

        // No exporter: the spans are needed for their ids only, not for what they are.
        let tracer_provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("test")))
            .with(build_logs_bridge(&logger_provider));

        // The container outlives the run, so a fixed id could be matched in an earlier run's
        // records and the assertion would hold whether or not this run's fields arrived.
        let chat_id = -i64::from(std::process::id());

        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("a_handler", chat_id);
            let _entered = span.enter();
            tracing::info!("a message from the test");
            // The shape sqlx logs a finished query in: fields only, no message.
            tracing::info!(summary = "SELECT Dicks …", rows_returned = 1);
            span.context().span().span_context().trace_id().to_string()
        });
        logger_provider.force_flush()
            .expect("couldn't flush the log records");

        let logs = query_logs(&base_url, &trace_id).await;
        assert!(logs.contains("a message from the test"), "the record is missing from:\n{logs}");
        assert!(logs.contains(&trace_id), "the trace id {trace_id} is missing from:\n{logs}");
        assert!(logs.contains(&chat_id.to_string()), "the chat_id of the span is missing from:\n{logs}");
        assert!(logs.contains(r#""severity_text":"INFO""#), "the level is missing from:\n{logs}");
        // A record without a message takes it from `summary`; see `build_logger_provider`.
        assert!(logs.contains(r#""_msg":"SELECT Dicks …""#), "the fallback message is missing from:\n{logs}");
        // The service alone identifies the stream; see `build_logger_provider`.
        assert!(logs.contains(r#""_stream":"{service.name=\"dick-grower-bot\"}""#),
            "unexpected stream fields in:\n{logs}");
    }

    /// Where to reach it, once it answers.
    async fn victoria_logs() -> String {
        let base_url = format!("http://localhost:{}", CONTAINER.port().await);

        // The container is up before the HTTP server inside it is.
        let client = reqwest::Client::new();
        for _ in 0..50 {
            let response = client.get(format!("{base_url}/health")).send().await;
            if response.is_ok_and(|r| r.status().is_success()) {
                return base_url;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("VictoriaLogs didn't become healthy in time");
    }

    /// Everything stored, as JSON lines, once `awaited` is among it.
    ///
    /// Ingestion is asynchronous, hence the retries — and the wait is for that needle rather than
    /// for any answer at all, because the container outlives the run: a non-empty answer may be
    /// entirely the previous run's records.
    async fn query_logs(base_url: &str, awaited: &str) -> String {
        let client = reqwest::Client::new();
        for _ in 0..50 {
            let body = client.get(format!("{base_url}/select/logsql/query?query=*"))
                .send().await.expect("couldn't query VictoriaLogs")
                .text().await.expect("couldn't read the answer of VictoriaLogs");
            if body.contains(awaited) {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("{awaited} didn't reach VictoriaLogs in time");
    }
}
