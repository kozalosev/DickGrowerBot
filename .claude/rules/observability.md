---
paths:
  - "src/observability.rs"
  - "src/errors/details.rs"
  - "src/errors/flat.rs"
  - "src/errors/handler.rs"
  - "src/errors/mod.rs"
  - "src/errors/moot.rs"
  - "src/metrics.rs"
  - "src/main.rs"
  - "src/scheduler/mod.rs"
  - "docker-compose.override.yml"
  - "docker-compose.yml"
---

## Observability / tracing

Logging and tracing go through `tracing` (initialized in `src/observability.rs` via
`observability::init_tracing()` in `main.rs`). The bot logs with `tracing::{info,warn,error,debug}!`
only; the `log::*` records of the libraries (teloxide, sqlx, reqwest) are captured by the
`tracing-log` bridge, so everything shares one pipeline.

```
RUST_LOG=info                                      # verbosity of the console and of the log export
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317  # spans, OTLP/gRPC; unset => spans are not exported
OTEL_EXPORTER_OTLP_LOGS_ENDPOINT=http://localhost:9428/insert/opentelemetry/v1/logs  # log records, OTLP/HTTP
OTEL_SPAN_FILTER=info,h2=off,hyper=off,tower=off,teloxide=info,reqwest=info,sqlx=off  # what becomes a span
OTEL_TRACES_SAMPLE_RATIO=1.0                       # the share of traces kept, parent-based
OTEL_BSP_QUEUE_SIZE=8192                           # spans held while waiting to be sent
OTEL_BSP_BATCH_SIZE=2048                           # spans per export
OTEL_BSP_DELAY=2s                                  # how often the queue drains
```

**`RUST_LOG` does not govern the spans.** The span layer has a filter of its own, and it used to be
a hardcoded `trace` — so `sqlx` streamed an event per query into every span, and one broadcast tick
turned a few hundred sends into tens of thousands of events, whatever `RUST_LOG` said. It is
`OTEL_SPAN_FILTER` now, defaulting to `info` with `sqlx=off`, and the two verbosities are separate
because they answer different questions: one is what a human reads, the other is how much of the
program's shape is worth keeping.

The bot's **per-item scheduler spans are written at `debug`** for the same reason
(`send_and_record`, `resolve_broadcast_language`): a run reaches every chat that is owed a summary,
so at `info` one midnight would be a few hundred thousand spans. `OTEL_SPAN_FILTER=debug` brings
them back, which is what to set while looking into a worker. The run-level spans stay at `info` —
there are only a few a minute, and they are what shows a tick as a whole.

**The sampling is parent-based**, so a decision taken at the root holds for every span beneath it
and a trace never arrives with holes. The unit being sampled is therefore a whole trace, and for a
scheduler that trace is **one tick of its loop** — at `0.05` one tick in twenty is kept entire,
rather than one span in twenty scattered across all of them.

The batch settings exist because the SDK's stock queue of 2048 is smaller than a single broadcast
tick, so the spans of a busy minute were dropped before the exporter thread woke up. Spans go out
gzipped (`gzip-tonic`).

**The layer is left off the subscriber entirely when `OTEL_EXPORTER_OTLP_ENDPOINT` is unset.** It
used to be attached regardless, building every span for a provider with no exporter behind it.

The console layer is **always** on and is the fallback: `docker logs` and journald keep working, and
it is what remains when the collector can't be reached. The two signals need two variables because
they go to different places and speak different protocols — the spans to Jaeger/Tempo over gRPC, the
records to VictoriaLogs over HTTP (the full URL, path included). The logs endpoint is always passed
to the exporter explicitly; left to itself it would fall back to the traces one and post log records
to the tracing backend.

Spans are exported over OTLP/gRPC (batch) when `OTEL_EXPORTER_OTLP_ENDPOINT` is set; the
service name is the crate name (`dick-grower-bot`). A trace of an update is rooted at its
`update` span (see below), and the handler that processes it is a child of that; outbound user-service calls (tonic) are auto-instrumented, and W3C trace-context
propagates to the user-service.

The HTTP server has **no** OpenTelemetry layer, on purpose: its only two routes are the webhook and
`/metrics`, and neither is worth a span. The webhook handler merely parses the update and puts it
into the dispatcher's queue — the work happens in another task, which the HTTP span can't reach
(teloxide passes the update through a plain channel, so the context is lost there anyway) — and
`/metrics` is Prometheus scraping us. The rate and the latency of both come from `axum-prometheus`.
If a route worth tracing ever appears, add `axum-tracing-opentelemetry` back for it.

The exported records carry the `trace_id`/`span_id` of the span they were written in — the SDK puts
them there, which is why the console lines have no ids: without the infrastructure there is nothing
to match them against. Records of the exporter's own stack (`opentelemetry`, `hyper`, `h2`, `tower`,
`reqwest`) are never exported: the exporter logs while it sends, and those records would be sent
again. `observability::tests` covers the whole path against a VictoriaLogs container.

**A record also carries the fields of every span it was written inside**, from the root down, the
nearer span winning where two of them name the same field. That is what makes "a log message is a
constant; the values are fields" pay off in the log database rather than only on the console: a
message that leaves `chat_id` out of its text is still searchable by it. It takes the
`experimental_span_attributes` feature of `opentelemetry-appender-tracing` and
`build_logs_bridge`; without them the SDK exports the two ids and nothing else, and every
`#[tracing::instrument(fields(…))]` in the bot is worth something to the traces and nothing to
VictoriaLogs. Every field is taken rather than a named few — a span's fields are already chosen by
hand at each `#[tracing::instrument]`, and an allowlist would be a second list to keep in step.

**A span decorates only what is logged while it is entered**, which is the catch for an error that
travels. A repository returns its errors instead of logging them, and teloxide's `ErrorHandler`
runs after the handler's span has closed and is given the error alone. So the whole tree is
wrapped in `errors::handler::trace_updates`, which runs every update inside an `update` span with
`update_id`, `chat_id`, `uid`, `chat_instance`, `inline_message_id` and `callback_data` read off the
`Update`, and logs a failed update in that span before it gets any further — `an update failed` —
passing nothing on to the dispatcher. One record per failure, the same fields whichever handler
failed. What it can't recover is the finer fields of the repo's own span (`topic_id`, a loan's id).

**The `update` span is what ties the records of one update together.** It is the root of the
trace, so every record written while the update is handled carries its fields and its trace id —
including the 90% of traces the sampler drops: a dropped span is still given a trace id, it is
only not exported. So the whole path of an update is `trace_id:=…` in VictoriaLogs whether or not
Jaeger has the trace, and everything a user did is `uid:=…`. The console has no trace ids, which is
what `update_id` is for.

**A plain group message — one that isn't our command — gets no span up front**, because a group the
bot reads in full sends plenty and most of them reach no handler. A branch that does act on them
(a title rename, a migration, a support reply) chains `errors::handler::open_update_span()` right
after its filter, so the span opens only once that branch has taken the update, and the handler's
spans sit under it like any other. `trace_updates` and the node share the span through a slot in
the dependencies, so a failure lands in the same trace. A branch that forgets the node still works:
its handler's spans become roots of their own, and a failure gets a span opened afterwards for the
failure record alone.

So **the text of an error is a constant too**, like a log message, and no value goes into it:

* **the chat a handler or a scheduler works in is never named at all** — the update's fields carry
  it, or the span of the scheduler that logs the failure (`send_and_record`, the cache reads). An
  internal id is that same chat in another form;
* **every other value goes through `.detailed_context(…)`** (`errors::details`):
  `Details::new("couldn't pay for the loan").subject_uid(uid).payout(value)`, or
  `.user_context(uid, "…")` for the user alone. A root error is `Details::new("…").count(x).into()` in place of `anyhow!` or
  `bail!` with a value. `trace_updates` collects the values from the whole cause chain, the
  outermost winning, and logs each as a field of its own name. The user is `subject_uid`: the
  update names only its sender, and a repository method is as often called for somebody else —
  the Dick of the Day's winner, the other side of a battle.

The names are a closed list, `error_fields!` in `errors::details`, because a `tracing` field's name
is fixed where it is written. One entry there is the field, its `Details` method and its place in
the record. A value that fits no name already on the list gets a new one, named for what it is
(`old_chat_id`, `merged_chat_id`), rather than squeezed into a near one. Every entry takes the
domain type of what it names, so a setter can't be handed the wrong kind of value — which is also
why a bet, a debt, a payout and a length change are fields of their own rather than one "amount".
`count` takes a `Count<T>` of anything and keeps the number. A list is logged joined by commas.

The values ride in an error of their own, `DetailedError`, because a handler's error has become a
`Box<dyn Error>` by the time it is logged and only an error in the chain can be downcast there.
**It is never the outermost one**: anyhow turns an `anyhow::Error` into a box of its own private
type, so the outermost error can't be downcast at all. So the message is an ordinary anyhow context
on top, the `DetailedError` sits under it with no text of its own, and `flat!` leaves out the links
that have none.

A failure handled in a span of its own doesn't go through `trace_updates`, so its user is
whatever that span or the log line names — `pvp` logs the winner and the loser beside a failed
battle result, for one.

An inline query and a chosen inline result carry no chat, so a failure there keeps the chat only
through `inline_message_id`, which encodes it in a supergroup.

`docker-compose.yml` bundles an **optional** observability stack — Jaeger for the spans,
VictoriaLogs for the records — gated behind the `tracing` Compose profile. The `infra`/`infra:full`
tasks start both (they name them, activating the profile) and `docker-compose.override.yml`
publishes their ports to `localhost` for the local-binary flow: Jaeger UI `http://localhost:16686`,
OTLP `localhost:4317`, VictoriaLogs UI `http://localhost:9428/select/vmui/` and ingestion on the
same port. For `task up` (skips the override) enable it with `COMPOSE_PROFILES=tracing`; there they
are network-internal and the in-Docker bot reaches them at `jaeger:4317` and `victoria-logs:9428`.
(`user-service` is likewise optional, behind the `user-service` profile.)

Aggregate function-level metrics (request rate / error rate / latency histograms) come from
[`autometrics`](https://docs.rs/autometrics): handlers and query-executing repo methods carry
`#[autometrics]` (paired with `#[tracing::instrument]`). The exporter is initialized
in `main.rs` (`autometrics::prometheus_exporter::init()`) and its output is appended to the
existing `/metrics` endpoint in `src/metrics.rs`, alongside the `axum-prometheus` and custom
counters — all scraped by Prometheus from the same port `8080` `/metrics` route.

**A panic is caught by a process-wide hook** (`observability::install_panic_hook`, called from
`main.rs` right after `init_tracing`), not left to Rust's default. The default hook writes straight
to stderr — outside `tracing` entirely, so a panic reached neither the console formatting nor the
OTLP log export nor any metric. That gap is what let the shrink broadcast worker's freeze (a
different failure, a hang rather than a panic) go unnoticed for hours in the first place, and it
would have hidden a panic just as well: nothing but the process staying up with one dead task to
show for it. The hook logs an `error!` (location, message, thread, backtrace) through the same
subscriber as everything else, and counts it in `panics_total{location}` — low-cardinality, since a
panic's location is a fixed point in the source, not user input. `DickGrowerBotPanicked` in
server-configs alerts on any nonzero rate.

**A scheduler tick recovers from its own panic instead of dying for good.** Every worker in
`scheduler.rs` is a fire-and-forget `tokio::spawn` with no `JoinHandle` kept anywhere — a panic that
unwinds past the loop ends that task silently, with nothing awaiting it to notice. `resilient()`
wraps one tick's work in `catch_unwind` (via `AssertUnwindSafe`, since the futures here borrow
`Repositories`/`Throttle<Bot>` across an `.await` that the compiler can't prove safe to resume) so
the loop gets another tick instead. The panic is still logged and counted by the hook above — this
only stops it from unwinding any further. Nothing here holds a lock that a caught panic could leave
half-updated: the state that matters lives in the database and Redis, outside this process.

