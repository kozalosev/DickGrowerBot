use std::error::Error;
use std::sync::Arc;
use futures::future::BoxFuture;
use teloxide::RequestError;
use teloxide::error_handlers::ErrorHandler;
use crate::metrics::TELEGRAM_REQUEST_ERRORS;

/// An [`ErrorHandler`] that logs like teloxide's `LoggingErrorHandler` but also feeds the
/// `telegram_request_errors_total` metric, classifying each [`RequestError`] by kind so that a
/// spike of `connect`/`timeout` errors (the DPI/ТСПУ signal) becomes visible in Prometheus.
///
/// It handles both error types that flow through the dispatcher: the boxed errors returned by
/// handlers (a [`RequestError`] — e.g. a failed `sendMessage` — is counted; anything else, like a
/// DB error, is only logged) and a bare [`RequestError`] from an update listener (e.g. failing
/// `getUpdates` when polling).
pub struct MetricsErrorHandler {
    text: String,
}

impl MetricsErrorHandler {
    pub fn new(text: impl Into<String>) -> Arc<Self> {
        Arc::new(Self { text: text.into() })
    }
}

impl ErrorHandler<Box<dyn Error + Send + Sync>> for MetricsErrorHandler {
    fn handle_error(self: Arc<Self>, error: Box<dyn Error + Send + Sync>) -> BoxFuture<'static, ()> {
        if let Some(request_error) = error.downcast_ref::<RequestError>() {
            TELEGRAM_REQUEST_ERRORS.record(classify(request_error));
        }
        log::error!("{}: {:?}", self.text, error);
        Box::pin(async {})
    }
}

impl ErrorHandler<RequestError> for MetricsErrorHandler {
    fn handle_error(self: Arc<Self>, error: RequestError) -> BoxFuture<'static, ()> {
        TELEGRAM_REQUEST_ERRORS.record(classify(&error));
        log::error!("{}: {:?}", self.text, error);
        Box::pin(async {})
    }
}

/// Classifies a [`RequestError`] into a low-cardinality metric label. Connection-phase failures are
/// checked before the timeout branch because a connect timeout satisfies both `is_connect()` and
/// `is_timeout()`, and we want it labeled `connect`.
fn classify(error: &RequestError) -> &'static str {
    match error {
        RequestError::Network(e) if e.is_connect() => "connect",
        RequestError::Network(e) if e.is_timeout() => "timeout",
        RequestError::Network(_) | RequestError::Io(_) => "network",
        RequestError::Api(_) | RequestError::RetryAfter(_) | RequestError::MigrateToChatId(_) => "api",
        _ => "other",
    }
}
