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
        tracing::error!(context = %self.text, error = %chain(error.as_ref()), "an error reached the error handler");
        Box::pin(async {})
    }
}

impl ErrorHandler<RequestError> for MetricsErrorHandler {
    fn handle_error(self: Arc<Self>, error: RequestError) -> BoxFuture<'static, ()> {
        TELEGRAM_REQUEST_ERRORS.record(classify(&error));
        tracing::error!(context = %self.text, error = %chain(&error), "an error reached the error handler");
        Box::pin(async {})
    }
}

/// Joins an error with everything that caused it into a single line, the way `anyhow`'s `{:#}`
/// does. The chain has to stay on one line: the log database stores a record per line, so a
/// multi-line error arrives as several records, and only the first of them carries the fields that
/// say where it came from.
fn chain(error: &(dyn Error + 'static)) -> String {
    let mut result = error.to_string();
    let mut cause = error.source();
    while let Some(e) = cause {
        result.push_str(": ");
        result.push_str(&e.to_string());
        cause = e.source();
    }
    result
}

/// Classifies a [`RequestError`] into a low-cardinality metric label. Connection-phase failures are
/// checked before the timeout branch because a connect timeout satisfies both `is_connect()` and
/// `is_timeout()`, and we want it labeled `connect`.
pub fn classify(error: &RequestError) -> &'static str {
    match error {
        RequestError::Network(e) if e.is_connect() => "connect",
        RequestError::Network(e) if e.is_timeout() => "timeout",
        RequestError::Network(_) | RequestError::Io(_) => "network",
        // Telegram says the bot sends too fast. Kept apart from the other API answers, because it
        // is the one that says the limits are too high rather than that the request was wrong.
        RequestError::RetryAfter(_) => "rate_limited",
        RequestError::Api(_) | RequestError::MigrateToChatId(_) => "api",
        _ => "other",
    }
}

#[cfg(test)]
mod test {
    use anyhow::Context;

    #[test]
    fn chain_stays_on_one_line() {
        let error: Box<dyn std::error::Error + Send + Sync> = Err::<(), _>(std::fmt::Error)
            .context("the inner context")
            .context("the outer context")
            .unwrap_err()
            .into();

        let line = super::chain(error.as_ref());

        assert_eq!(line, "the outer context: the inner context: an error occurred when formatting an argument");
    }
}
