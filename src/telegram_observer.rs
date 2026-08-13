use std::borrow::Cow;
use std::time::Duration;
use teloxide::observer::{RequestBody, RequestObservation, RequestObserver};
use teloxide::{ApiError, RequestError};
use tracing::Span;
use crate::error_handler::classify;
use crate::metrics::{TELEGRAM_REQUEST_DURATION, TELEGRAM_REQUEST_ERRORS};

/// Watches every request the bot sends to the Telegram Bot API and turns it into three signals:
/// the `telegram_request_duration_seconds` histogram, a client span the request's latency can be
/// seen in, and — when the API rejects a request it doesn't have a proper error code for — a log
/// record carrying the payload that caused it.
///
/// It is attached to the bot in [`crate::config::BotConfig::build_bot`] and sits below the
/// adaptors, so the time [`teloxide::adaptors::Throttle`] spends holding a request back is not
/// counted as request time.
pub struct TelegramObserver;

/// A request in flight: what was sent, and the span that stays open until the answer arrives.
pub struct PendingRequest {
    method: &'static str,
    body: Option<Vec<u8>>,
    span: Span,
}

impl RequestObserver for TelegramObserver {
    fn on_request(&self, method: &'static str, body: RequestBody<'_>) -> Box<dyn RequestObservation> {
        let span = tracing::info_span!("telegram_request",
            otel.kind = "client", rpc.system = "telegram", rpc.method = method);
        Box::new(PendingRequest {
            method,
            body: body.json().map(<[u8]>::to_vec),
            span,
        })
    }
}

impl RequestObservation for PendingRequest {
    fn on_response(self: Box<Self>, elapsed: Duration, outcome: Result<(), &RequestError>) {
        let label = outcome.map_or_else(classify, |()| "ok");
        TELEGRAM_REQUEST_DURATION.record(self.method, label, elapsed.as_secs_f64());
        // One classification feeds both, so the histogram's `outcome` and the counter's `kind` can
        // never disagree. Counted here rather than at the dispatcher because this sits below every
        // adaptor, and so sees the schedulers' requests too — which the dispatcher never does.
        if outcome.is_err() {
            TELEGRAM_REQUEST_ERRORS.record(label);
        }

        // `ApiError::Unknown` is the answer Telegram gives when it dislikes something about the
        // payload itself — a broken entity, an over-long text — and the message alone never says
        // which part. The body is the only way to find out, so it is logged with it.
        if let Err(err @ RequestError::Api(ApiError::Unknown(_))) = outcome {
            let payload = self.body.as_deref()
                .map(String::from_utf8_lossy)
                .unwrap_or(Cow::Borrowed("<multipart>"));
            self.span.in_scope(|| tracing::error!(method = %self.method, payload = %payload,
                error = %err, "the Telegram API rejected the request"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use teloxide::observer::{RequestBody, RequestObserver};
    use teloxide::{ApiError, RequestError};
    use crate::metrics::render_metrics;
    use crate::telegram_observer::TelegramObserver;

    /// Asserts that the histogram holds exactly `count` samples for these labels. Every test uses
    /// a method name of its own, so the series don't collide when the tests run in parallel.
    fn assert_samples(method: &str, outcome: &str, count: u64) {
        let series = format!(
            "telegram_request_duration_seconds_count{{method=\"{method}\",outcome=\"{outcome}\"}} {count}");
        let rendered = render_metrics();
        assert!(rendered.contains(&series), "{series} is missing from:\n{rendered}");
    }

    #[test]
    fn records_the_duration_of_a_successful_request() {
        let guard = TelegramObserver.on_request("GetMe", RequestBody::Json(b"{}"));
        guard.on_response(Duration::from_millis(120), Ok(()));

        assert_samples("GetMe", "ok", 1);
    }

    /// `telegram_request_errors_total` carries only a `kind`, so unlike the histogram there is no
    /// per-test label to hide behind while the tests run in parallel. The delta is what can be
    /// asserted; the absolute value belongs to whatever else ran at the same time.
    fn errors_of(kind: &str) -> u64 {
        let series = format!("telegram_request_errors_total{{kind=\"{kind}\"}} ");
        render_metrics().lines()
            .find_map(|line| line.strip_prefix(&series))
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0)
    }

    #[test]
    fn records_a_failed_request_under_its_own_outcome() {
        let err = RequestError::Api(ApiError::BotBlocked);
        let before = errors_of("api");

        let guard = TelegramObserver.on_request("SendDice", RequestBody::Json(b"{}"));
        guard.on_response(Duration::from_millis(50), Err(&err));

        assert_samples("SendDice", "api", 1);
        // The same classification feeds both, which is the point of counting the errors here: the
        // schedulers never reach a dispatcher, so nothing else would count theirs.
        assert!(errors_of("api") > before, "the failed request wasn't counted as an error");
    }

    /// A request that went through must leave the error counter alone, or every graph of the error
    /// rate would follow the traffic instead of the failures.
    #[test]
    fn a_successful_request_is_not_counted_as_an_error() {
        let before = errors_of("network");

        let guard = TelegramObserver.on_request("GetChat", RequestBody::Json(b"{}"));
        guard.on_response(Duration::from_millis(10), Ok(()));

        assert_samples("GetChat", "ok", 1);
        assert_eq!(errors_of("network"), before);
    }

    /// A multipart request has no body to log, but it must still be measured.
    #[test]
    fn measures_a_multipart_request_too() {
        let guard = TelegramObserver.on_request("SendPhoto", RequestBody::Multipart);
        guard.on_response(Duration::from_secs(2), Ok(()));

        assert_samples("SendPhoto", "ok", 1);
    }

    /// The payload of a rejected request is logged; the record itself goes to the subscriber, so
    /// what is checked here is that the path runs and the request is still measured.
    #[test]
    fn handles_a_rejected_payload() {
        let err = RequestError::Api(ApiError::Unknown("Bad Request: can't parse entities".to_owned()));

        let guard = TelegramObserver.on_request("SendMessage", RequestBody::Json(br#"{"text":"<b>"}"#));
        guard.on_response(Duration::from_millis(30), Err(&err));

        assert_samples("SendMessage", "api", 1);
    }
}
