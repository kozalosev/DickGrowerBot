use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ops::ControlFlow;
use std::sync::{Arc, OnceLock};
use futures::future::BoxFuture;
use teloxide::RequestError;
use teloxide::dispatching::{DpHandlerDescription, UpdateHandler};
use teloxide::dptree::{HandlerDescription, HandlerSignature};
use teloxide::dptree::di::DependencyMap;
use teloxide::error_handlers::ErrorHandler;
use teloxide::types::{Update, UpdateKind};
use tracing::{Instrument, Span};
use crate::errors::details::ErrorFields;
use crate::errors::flat::flat;
use crate::handlers::checks::is_our_command;
use crate::domain::objects::BotNames;

type HandlerError = Box<dyn Error + Send + Sync>;

/// Runs every update inside a span of its own, the root of its trace, with the ids of the update as
/// fields, so every record written while it is handled carries them and the trace id. A failure is
/// logged in that span too: the handlers' spans have closed by the time an error reaches the
/// dispatcher's [`ErrorHandler`], and that handler is given the error alone.
///
/// A message in a group that isn't one of our commands gets no span up front, since most of them
/// reach no handler. A branch that acts on such messages chains [`open_update_span`] after its
/// filter, and the span is opened there. Should handling fail with no span opened, one is opened
/// afterwards for the failure alone.
///
/// The error is logged here and not passed on, so the dispatcher's handler sees none of them.
pub fn trace_updates(handler: UpdateHandler<HandlerError>) -> UpdateHandler<HandlerError> {
    let description = handler.description().clone();
    let signature = handler.sig().clone();
    teloxide::dptree::from_fn_with_description(description, move |mut deps: DependencyMap, cont| {
        let handler = handler.clone();
        let update = deps.try_get::<Update>();
        let names = deps.try_get::<BotNames>();
        let slot = UpdateSpan::default();
        let ignored = update.as_deref()
            .is_some_and(|update| is_ignored_group_message(update, names.as_deref()));
        let span = if ignored { Span::none() } else { slot.open(update.as_deref()) };
        deps.insert(slot.clone());
        async move {
            match handler.execute(deps, cont).instrument(span).await {
                ControlFlow::Break(Err(error)) => {
                    let span = slot.open(update.as_deref());
                    span.in_scope(|| ErrorFields::of(error.as_ref()).log_failed_update(error.as_ref()));
                    ControlFlow::Break(Ok(()))
                }
                other => other,
            }
        }
    }, signature)
}

/// Runs the rest of the branch inside the `update` span, opening it if nothing has yet.
pub fn open_update_span() -> UpdateHandler<HandlerError> {
    let signature = HandlerSignature::Other {
        obligations: BTreeMap::new(),
        guaranteed_outcomes: BTreeSet::new(),
        conditional_outcomes: BTreeSet::new(),
        continues: true,
    };
    teloxide::dptree::from_fn_with_description(DpHandlerDescription::entry(), |deps: DependencyMap, cont| async move {
        match deps.try_get::<UpdateSpan>() {
            Some(slot) => {
                let span = slot.open(deps.try_get::<Update>().as_deref());
                cont(deps).instrument(span).await
            }
            None => cont(deps).await,
        }
    }, signature)
}

/// An [`ErrorHandler`] that logs like teloxide's `LoggingErrorHandler`, but joins the whole cause
/// chain onto one line and says which part of the dispatcher the error came from.
///
/// It handles both error types that flow through the dispatcher: the boxed errors of the handlers,
/// for a tree that isn't wrapped in [`trace_updates`], and a bare [`RequestError`] from an
/// update listener (e.g. failing `getUpdates` when polling). Neither is counted here — `telegram_request_errors_total` is written by
/// [`crate::telegram_observer`], which sees every request rather than only the ones that reach a
/// dispatcher.
pub struct ContextLoggingErrorHandler {
    text: String,
}

impl ContextLoggingErrorHandler {
    pub fn new(text: impl Into<String>) -> Arc<Self> {
        Arc::new(Self { text: text.into() })
    }
}

impl ErrorHandler<HandlerError> for ContextLoggingErrorHandler {
    fn handle_error(self: Arc<Self>, error: HandlerError) -> BoxFuture<'static, ()> {
        tracing::error!(context = %self.text, error = flat!(error), "an error reached the error handler");
        Box::pin(async {})
    }
}

impl ErrorHandler<RequestError> for ContextLoggingErrorHandler {
    fn handle_error(self: Arc<Self>, error: RequestError) -> BoxFuture<'static, ()> {
        tracing::error!(context = %self.text, error = flat!(error), "an error reached the error handler");
        Box::pin(async {})
    }
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

/// A message in a group that isn't one of our commands. Without the bot's names, no command can
/// be told apart, so nothing is ignored.
fn is_ignored_group_message(update: &Update, names: Option<&BotNames>) -> bool {
    let UpdateKind::Message(msg) = &update.kind else {
        return false
    };
    let in_group = msg.chat.is_group() || msg.chat.is_supergroup();
    in_group && names.is_some_and(|names| !is_our_command(msg, names))
}

/// The `update` span of the update being handled, once something has opened it.
#[derive(Clone, Default)]
struct UpdateSpan(Arc<OnceLock<Span>>);

impl UpdateSpan {
    fn open(&self, update: Option<&Update>) -> Span {
        self.0.get_or_init(|| update_span(update)).clone()
    }
}

fn update_span(update: Option<&Update>) -> Span {
    let update_id = update.map(|update| update.id.0);
    let chat_id = update.and_then(Update::chat).map(|chat| chat.id.0);
    let uid = update.and_then(Update::from).map(|user| user.id.0);
    let (chat_instance, inline_message_id, callback_data) = match update.map(|update| &update.kind) {
        Some(UpdateKind::CallbackQuery(query)) => (
            Some(query.chat_instance.as_str()),
            query.inline_message_id.as_deref(),
            query.data.as_deref(),
        ),
        Some(UpdateKind::ChosenInlineResult(result)) => (None, result.inline_message_id.as_deref(), None),
        _ => (None, None, None),
    };
    tracing::info_span!("update", update_id, chat_id, uid, chat_instance, inline_message_id, callback_data)
}

#[cfg(test)]
mod test {
    use std::io::Write;
    use std::ops::ControlFlow;
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use teloxide::dispatching::UpdateHandler;
    use tracing::Subscriber;
    use tracing::span::{Attributes, Id};
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use teloxide::dptree;
    use teloxide::types::{Update, UpdateKind};
    use crate::domain::primitives::{LengthChange, UserId, Username};
    use crate::domain::objects::BotNames;
    use crate::errors::details::{DetailedContext, Details};
    use super::HandlerError;

    #[test]
    fn a_failed_update_is_logged_with_its_ids() {
        let failing: UpdateHandler<HandlerError> = dptree::endpoint(|| async {
            Err::<(), HandlerError>("the inner error".into())
        });

        let (result, logs) = dispatch(super::trace_updates(failing));

        assert!(matches!(result, ControlFlow::Break(Ok(()))));
        assert!(logs.contains("an update failed"), "{logs}");
        assert!(logs.contains("chat_id=-1001234567890"), "{logs}");
        assert!(logs.contains("uid=42"), "{logs}");
        assert!(logs.contains("error=the inner error"), "{logs}");
    }

    #[test]
    fn the_values_an_error_names_are_logged_as_fields() {
        let failing: UpdateHandler<HandlerError> = dptree::endpoint(|| async {
            Err::<(), _>(std::fmt::Error)
                .detailed_context(Details::new("couldn't update the length")
                    .subject_uid(UserId::new(7))
                    .length_change(LengthChange::signed(-3)))?;
            Ok(())
        });

        let (_, logs) = dispatch(super::trace_updates(failing));

        assert!(logs.contains("uid=42"), "{logs}");
        assert!(logs.contains("subject_uid=7"), "{logs}");
        assert!(logs.contains("length_change=-3"), "{logs}");
        assert!(logs.contains("error=couldn't update the length: an error"), "{logs}");
    }

    #[test]
    fn what_a_handler_logs_carries_the_ids_of_the_update() {
        let logging: UpdateHandler<HandlerError> = dptree::endpoint(|| async {
            tracing::info!("handled");
            Ok(())
        });

        let (_, logs) = dispatch(super::trace_updates(logging));

        assert!(logs.contains("update_id=1"), "{logs}");
        assert!(logs.contains("chat_id=-1001234567890"), "{logs}");
        assert!(logs.contains("uid=42"), "{logs}");
    }

    #[test]
    fn a_plain_group_message_gets_no_span_up_front() {
        let logging: UpdateHandler<HandlerError> = dptree::endpoint(|| async {
            tracing::info!("handled");
            Ok(())
        });

        let (_, logs) = dispatch_text(super::trace_updates(logging), "hello");

        assert!(logs.contains("handled"), "{logs}");
        assert!(!logs.contains("update_id"), "{logs}");
    }

    #[test]
    fn a_branch_may_open_the_span_for_a_plain_group_message() {
        let logging: UpdateHandler<HandlerError> = super::open_update_span().endpoint(|| async {
            tracing::info!("handled");
            Ok(())
        });

        let (_, logs, spans) = run(super::trace_updates(logging), "hello");

        assert!(logs.contains("update_id=1"), "{logs}");
        assert_eq!(spans, 1);
    }

    #[test]
    fn a_failure_after_the_span_was_opened_is_logged_in_it() {
        let failing: UpdateHandler<HandlerError> = super::open_update_span().endpoint(|| async {
            tracing::info!("handled");
            Err::<(), HandlerError>("the inner error".into())
        });

        let (_, logs, spans) = run(super::trace_updates(failing), "hello");

        assert!(logs.contains("handled"), "{logs}");
        assert!(logs.contains("an update failed"), "{logs}");
        assert_eq!(logs.matches("update_id=1").count(), 2, "{logs}");
        assert_eq!(spans, 1);
    }

    #[test]
    fn a_later_branch_runs_inside_the_span_an_earlier_one_opened() {
        let tree: UpdateHandler<HandlerError> = dptree::entry()
            .branch(super::open_update_span().inspect(|| tracing::info!("passed on")))
            .branch(super::open_update_span().endpoint(|| async {
                tracing::info!("handled");
                Ok(())
            }));

        let (_, logs, spans) = run(super::trace_updates(tree), "hello");

        assert_eq!(logs.matches("update_id=1").count(), 2, "{logs}");
        assert_eq!(spans, 1);
    }

    #[test]
    fn opening_the_span_of_a_command_again_changes_nothing() {
        let logging: UpdateHandler<HandlerError> = super::open_update_span().endpoint(|| async {
            tracing::info!("handled");
            Ok(())
        });

        let (_, logs, spans) = run(super::trace_updates(logging), "/grow");

        assert_eq!(logs.matches("update{").count(), 1, "{logs}");
        assert_eq!(spans, 1);
    }

    #[test]
    fn a_failed_plain_group_message_is_still_logged_with_its_ids() {
        let failing: UpdateHandler<HandlerError> = dptree::endpoint(|| async {
            Err::<(), HandlerError>("the inner error".into())
        });

        let (_, logs) = dispatch_text(super::trace_updates(failing), "hello");

        assert!(logs.contains("an update failed"), "{logs}");
        assert!(logs.contains("update_id=1"), "{logs}");
        assert!(logs.contains("uid=42"), "{logs}");
    }

    #[test]
    fn a_successful_update_is_left_alone() {
        let succeeding: UpdateHandler<HandlerError> = dptree::endpoint(|| async { Ok(()) });

        let (result, logs) = dispatch(super::trace_updates(succeeding));

        assert!(matches!(result, ControlFlow::Break(Ok(()))));
        assert!(logs.is_empty(), "{logs}");
    }

    fn dispatch(handler: UpdateHandler<HandlerError>) -> (ControlFlow<Result<(), HandlerError>, ()>, String) {
        dispatch_text(handler, "/grow")
    }

    fn dispatch_text(handler: UpdateHandler<HandlerError>, text: &str) -> (ControlFlow<Result<(), HandlerError>, ()>, String) {
        let (result, logs, _) = run(handler, text);
        (result, logs)
    }

    /// Dispatches a message with the given text and returns the outcome, the logs and how many
    /// `update` spans were created.
    fn run(handler: UpdateHandler<HandlerError>, text: &str) -> (ControlFlow<Result<(), HandlerError>, ()>, String, usize) {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        let spans = UpdateSpanCounter::default();
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone()))
            .with(spans.clone());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build().expect("couldn't build a runtime");

        let result = tracing::subscriber::with_default(subscriber, || {
            runtime.block_on(handler.dispatch(dptree::deps![update(text), names()]))
        });
        let logs = String::from_utf8(buffer.0.lock().unwrap().clone()).expect("the logs are not UTF-8");
        (result.map_continue(|_| ()), logs, spans.0.load(Ordering::SeqCst))
    }

    fn update(text: &str) -> Update {
        let json = r#"{
            "update_id": 1,
            "message": {
                "message_id": 1,
                "date": 0,
                "chat": { "id": -1001234567890, "type": "supergroup", "title": "Test", "is_forum": false },
                "from": { "id": 42, "is_bot": false, "first_name": "Tester" },
                "text": "TEXT"
            }
        }"#.replace("TEXT", text);
        let update: Update = serde_json::from_str(&json).expect("couldn't build the update");
        assert!(matches!(update.kind, UpdateKind::Message(_)), "{:?}", update.kind);
        update
    }

    fn names() -> BotNames {
        BotNames::new(Username::from("DickGrowerBot"), [])
    }

    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct UpdateSpanCounter(Arc<AtomicUsize>);

    impl<S: Subscriber> Layer<S> for UpdateSpanCounter {
        fn on_new_span(&self, attrs: &Attributes<'_>, _: &Id, _: Context<'_, S>) {
            if attrs.metadata().name() == "update" {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}
