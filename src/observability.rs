use std::error::Error;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_otlp::{LogExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::filter::{filter_fn, FilterExt};
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const SERVICE_NAME: &str = env!("CARGO_PKG_NAME");

const TRACES_ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const LOGS_ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT";

/// Records of these crates are never exported. The exporter logs while it sends, and those records
/// would be sent again — a loop that feeds itself. They still go to the console.
const NEVER_EXPORTED_TARGETS: [&str; 5] = ["opentelemetry", "hyper", "h2", "tower", "reqwest"];

/// The providers to shut down before the process exits, so the last batch reaches the collector.
pub struct Telemetry {
    tracer_provider: SdkTracerProvider,
    logger_provider: Option<SdkLoggerProvider>,
}

impl Telemetry {
    pub fn shutdown(&self) -> Result<(), Box<dyn Error>> {
        self.tracer_provider.shutdown()?;
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
/// - `RUST_LOG`: verbosity of both the console and the exported records;
/// - `OTEL_EXPORTER_OTLP_ENDPOINT`: where the spans go, over gRPC. Unset => spans are not exported;
/// - `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`: where the log records go, over HTTP (the full URL, e.g.
///   `http://victoria-logs:9428/insert/opentelemetry/v1/logs`). Unset => the console only.
///
/// The two signals need two variables because they usually live in different places and speak
/// different protocols. Trace and span ids are attached to the exported records by the SDK itself,
/// which is why the console lines carry no ids: without the infrastructure there is nothing to
/// match them against anyway.
pub fn init_tracing() -> Result<Telemetry, Box<dyn Error>> {
    let spans_exported = endpoint(TRACES_ENDPOINT_VAR).is_some();
    let tracer_provider = build_tracer_provider()?;
    global::set_tracer_provider(tracer_provider.clone());
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    // Suppress noisy internals at the OTel level; console verbosity is controlled by RUST_LOG
    let otel_filter = EnvFilter::new("trace,h2=off,hyper=off,tower=off,teloxide=info,reqwest=info");
    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer_provider.tracer(SERVICE_NAME))
        .with_filter(otel_filter);

    let logger_provider = build_logger_provider()?;
    let logs_layer = logger_provider.as_ref().map(|provider| {
        let filter = EnvFilter::from_default_env()
            .and(filter_fn(|metadata| !NEVER_EXPORTED_TARGETS.iter()
                .any(|target| metadata.target().starts_with(target))));
        OpenTelemetryTracingBridge::new(provider).with_filter(filter)
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

fn build_tracer_provider() -> Result<SdkTracerProvider, Box<dyn Error>> {
    let Some(endpoint) = endpoint(TRACES_ENDPOINT_VAR) else {
        return Ok(SdkTracerProvider::builder().build());
    };

    let otlp_exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource())
        .build())
}

/// The endpoint is always passed explicitly: left to itself, the exporter would fall back to
/// `OTEL_EXPORTER_OTLP_ENDPOINT` and send the log records to the tracing backend.
fn build_logger_provider() -> Result<Option<SdkLoggerProvider>, Box<dyn Error>> {
    let Some(endpoint) = endpoint(LOGS_ENDPOINT_VAR) else {
        return Ok(None);
    };

    let otlp_exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()?;
    Ok(Some(SdkLoggerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource())
        .build()))
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
