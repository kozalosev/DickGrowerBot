use std::fmt;
use std::sync::{Arc, OnceLock};
use opentelemetry::global;
use opentelemetry::trace::{SpanContext, TraceContextExt, TracerProvider};
use opentelemetry_sdk::Resource;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing::{Dispatch, Event, Subscriber};
use tracing::dispatcher::WeakDispatch;
use tracing_opentelemetry::get_otel_context;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

const SERVICE_NAME: &str = env!("CARGO_PKG_NAME");

/// Initialize tracing subscriber, optionally with OpenTelemetry OTLP export.
///
/// The `log::*` records of the libraries (teloxide, sqlx, reqwest) are bridged into the tracing
/// pipeline by `tracing_subscriber`'s built-in `tracing-log` feature, installed inside `try_init`,
/// so they end up in the same output as our own events.
///
/// Configuration via environment variables:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint. If unset, OTLP export is
///   disabled and only console output is produced (useful for local development).
/// - `RUST_LOG`: console log level filter
pub fn init_tracing() -> Result<SdkTracerProvider, Box<dyn std::error::Error>> {
    let provider = build_provider()?;
    global::set_tracer_provider(provider.clone());
    global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());

    // Suppress noisy internals at the OTel level; console verbosity is controlled by RUST_LOG
    let otel_filter = EnvFilter::new("trace,h2=off,hyper=off,tower=off,teloxide=info,reqwest=info");
    let telemetry_layer = tracing_opentelemetry::layer()
        .with_tracer(provider.tracer(SERVICE_NAME))
        .with_filter(otel_filter);
    let event_format = tracing_subscriber::fmt::format()
        .with_target(true)
        .with_line_number(true);
    let dispatch = Arc::new(OnceLock::new());
    let fmt_layer = tracing_subscriber::fmt::layer()
        .event_format(FormatterWithTraceIds { inner: event_format, dispatch: dispatch.clone() })
        .with_filter(EnvFilter::from_default_env());
    tracing_subscriber::registry()
        .with(telemetry_layer)
        .with(fmt_layer)
        .try_init()?;

    // Only now, with the subscriber installed, can the formatter be given the dispatcher it needs
    // to resolve trace ids (see [`WithTraceIds::dispatch`]).
    let _ = dispatch.set(tracing::dispatcher::get_default(Dispatch::downgrade));

    tracing::info!(service_name = %SERVICE_NAME, "Tracing initialized");
    Ok(provider)
}

/// Adds `trace_id=… span_id=…` at the end of every console line, taken from the OpenTelemetry
/// context of the span the log record was written in. This is what connects a log line with its
/// trace: the id can be pasted into Tempo/Jaeger, and Grafana turns it into a link on its own
/// (the VictoriaLogs data source looks for exactly this `trace_id=<32 hex chars>` pattern).
///
/// Lines written outside any span (startup, background tasks) simply have no ids to add.
struct FormatterWithTraceIds<F> {
    inner: F,
    /// The subscriber the formatter belongs to, needed to look the OpenTelemetry context up.
    ///
    /// The usual `Span::current().context()` doesn't work here: `tracing` refuses to hand the
    /// current dispatcher out while we are inside one of the subscriber's own callbacks (that's how
    /// it stops a subscriber from calling itself), so `Span::current()` is always a no-op span
    /// during formatting and its context is empty. Hence th,e dispatcher is remembered on
    /// initialization instead — see [`init_tracing`], which fills this in right after `try_init`.
    ///
    /// The reference is weak on purpose: the subscriber owns this formatter, and a strong one would
    /// make a cycle that never gets freed.
    dispatch: Arc<OnceLock<WeakDispatch>>,
}

impl<F> FormatterWithTraceIds<F> {
    /// The OpenTelemetry context of the span the event happened in, if there is one and if it has
    /// valid ids (nothing to show before the subscriber is installed or outside any span).
    fn span_context<S, N>(&self, ctx: &FmtContext<'_, S, N>) -> Option<SpanContext>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
        N: for<'a> FormatFields<'a> + 'static,
    {
        let dispatch = self.dispatch.get()?.upgrade()?;
        let span = ctx.lookup_current()?;
        let context = get_otel_context(&span.id(), &dispatch)?;
        let span_context = context.span().span_context().clone();
        span_context.is_valid().then_some(span_context)
    }
}

impl<S, N, F> FormatEvent<S, N> for FormatterWithTraceIds<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // Read the ids first: both this and the inner formatter lock the span's extensions, and
        // holding one of those locks while taking the other is exactly how a deadlock happens.
        let span_context = self.span_context(ctx);

        // The inner formatter writes a whole line, the trailing newline included, so it goes into a
        // buffer to let the ids be appended after the message instead of in front of the timestamp.
        let mut line = String::new();
        self.inner.format_event(ctx, Writer::new(&mut line), event)?;
        write!(writer, "{}", line.trim_end_matches('\n'))?;

        if let Some(span_context) = span_context {
            write!(writer, " trace_id={} span_id={}", span_context.trace_id(), span_context.span_id())?;
        }
        writeln!(writer)
    }
}

fn build_provider() -> Result<SdkTracerProvider, Box<dyn std::error::Error>> {
    let Some(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok() else {
        tracing::warn!("OTEL_EXPORTER_OTLP_ENDPOINT is not set — OTLP export disabled");
        return Ok(SdkTracerProvider::builder().build());
    };

    let otlp_exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    let resource = Resource::builder()
        .with_service_name(SERVICE_NAME.to_owned())
        .build();
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource)
        .build())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex, OnceLock};
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing::Dispatch;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::{layer::SubscriberExt, Layer};
    use super::FormatterWithTraceIds;

    /// Collects the formatted lines instead of printing them.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Buffer {
        fn contents(&self) -> String {
            let bytes = self.0.lock().expect("the buffer is poisoned").clone();
            String::from_utf8(bytes).expect("the log output is not valid UTF-8")
        }
    }

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("the buffer is poisoned").write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.0.lock().expect("the buffer is poisoned").flush()
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Logs `message`, inside a span when `in_span` is set, and returns what the console got.
    fn log_line(in_span: bool) -> String {
        let provider = SdkTracerProvider::builder().build();
        let telemetry_layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("test"));
        let buffer = Buffer::default();
        let dispatch = Arc::new(OnceLock::new());
        let fmt_layer = tracing_subscriber::fmt::layer()
            .event_format(FormatterWithTraceIds {
                inner: tracing_subscriber::fmt::format(),
                dispatch: dispatch.clone(),
            })
            .with_ansi(false)
            .with_writer(buffer.clone())
            .with_filter(tracing_subscriber::filter::LevelFilter::INFO);
        let subscriber = tracing_subscriber::registry()
            .with(telemetry_layer)
            .with(fmt_layer);

        tracing::subscriber::with_default(subscriber, || {
            // The same handover as in `init_tracing`, but for the scoped subscriber of this test.
            let _ = dispatch.set(tracing::dispatcher::get_default(Dispatch::downgrade));
            if in_span {
                let span = tracing::info_span!("a_handler");
                let _entered = span.enter();
                tracing::info!("a message");
            } else {
                tracing::info!("a message");
            }
        });
        buffer.contents()
    }

    /// The ids must be the last thing on the line and shaped exactly as Grafana's data source
    /// expects them (`trace_id=` followed by 32 hex characters), or the link to Tempo won't appear.
    #[test]
    fn the_ids_are_appended_to_a_line_written_in_a_span() {
        let line = log_line(true);
        let ids = line.trim_end()
            .split_once(" trace_id=")
            .unwrap_or_else(|| panic!("no trace_id in the line: {line}"));

        assert!(ids.0.ends_with("a message"), "the ids must come after the message, got: {line}");
        let (trace_id, span_id) = ids.1.split_once(" span_id=")
            .expect("no span_id in the line");
        assert_eq!(trace_id.len(), 32, "unexpected trace_id: {trace_id}");
        assert_eq!(span_id.len(), 16, "unexpected span_id: {span_id}");
        assert!(trace_id.chars().all(|c| c.is_ascii_hexdigit()), "unexpected trace_id: {trace_id}");
        assert!(span_id.chars().all(|c| c.is_ascii_hexdigit()), "unexpected span_id: {span_id}");
    }

    #[test]
    fn a_line_written_outside_of_a_span_is_left_alone() {
        let line = log_line(false);

        assert!(line.contains("a message"), "the message is missing: {line}");
        assert!(!line.contains("trace_id="), "unexpected ids: {line}");
    }
}
