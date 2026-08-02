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

    let logger_provider = endpoint(LOGS_ENDPOINT_VAR)
        .map(build_logger_provider)
        .transpose()?;
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
fn build_logger_provider(endpoint: String) -> Result<SdkLoggerProvider, Box<dyn Error>> {
    let otlp_exporter = LogExporter::builder()
        .with_http()
        .with_endpoint(endpoint)
        .build()?;
    Ok(SdkLoggerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource())
        .build())
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
    use testcontainers::{ContainerAsync, GenericImage};
    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::runners::AsyncRunner;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;
    use super::*;

    const VICTORIA_LOGS_PORT: u16 = 9428;
    const INGESTION_PATH: &str = "/insert/opentelemetry/v1/logs";

    /// The record must carry the ids of the span it was written in, put there by the SDK — that is
    /// the reason the bot has no formatter of its own for them anymore. Everything here is the real
    /// pipeline: the tracing bridge, the OTLP/HTTP exporter, and the same log database the server
    /// runs. The fields must survive as fields, not as text inside the message.
    #[tokio::test]
    async fn an_exported_record_carries_the_trace_id_and_the_fields() {
        let (_container, base_url) = start_victoria_logs().await;
        let logger_provider = build_logger_provider(format!("{base_url}{INGESTION_PATH}"))
            .expect("couldn't build the logger provider");

        // No exporter: the spans are needed for their ids only, not for what they are.
        let tracer_provider = SdkTracerProvider::builder().build();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_opentelemetry::layer().with_tracer(tracer_provider.tracer("test")))
            .with(OpenTelemetryTracingBridge::new(&logger_provider));

        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("a_handler");
            let _entered = span.enter();
            tracing::info!(chat_id = -100500, "a message from the test");
            span.context().span().span_context().trace_id().to_string()
        });
        logger_provider.force_flush()
            .expect("couldn't flush the log records");

        let logs = query_logs(&base_url).await;
        assert!(logs.contains("a message from the test"), "the record is missing from:\n{logs}");
        assert!(logs.contains(&trace_id), "the trace id {trace_id} is missing from:\n{logs}");
        assert!(logs.contains("-100500"), "the chat_id field is missing from:\n{logs}");
    }

    async fn start_victoria_logs() -> (ContainerAsync<GenericImage>, String) {
        let container = GenericImage::new("victoriametrics/victoria-logs", "latest")
            .with_exposed_port(VICTORIA_LOGS_PORT.tcp())
            .with_wait_for(WaitFor::millis(500))
            .start()
            .await
            .expect("couldn't start VictoriaLogs");
        let port = container.get_host_port_ipv4(VICTORIA_LOGS_PORT)
            .await
            .expect("couldn't fetch the port of VictoriaLogs");
        let base_url = format!("http://localhost:{port}");

        // The container is up before the HTTP server inside it is.
        let client = reqwest::Client::new();
        for _ in 0..50 {
            let response = client.get(format!("{base_url}/health")).send().await;
            if response.is_ok_and(|r| r.status().is_success()) {
                return (container, base_url);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("VictoriaLogs didn't become healthy in time");
    }

    /// Everything stored, as JSON lines. Ingestion is asynchronous, hence the retries.
    async fn query_logs(base_url: &str) -> String {
        let client = reqwest::Client::new();
        for _ in 0..50 {
            let body = client.get(format!("{base_url}/select/logsql/query?query=*"))
                .send()
                .await
                .expect("couldn't query VictoriaLogs")
                .text()
                .await
                .expect("couldn't read the answer of VictoriaLogs");
            if !body.trim().is_empty() {
                return body;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("nothing was stored in VictoriaLogs in time");
    }
}
