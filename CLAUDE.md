# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# DickGrowerBot — Claude Code Guide

## Build & Run

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests (requires Docker — testcontainers spins up a throwaway Postgres)
cargo test

# Run a single test (substring-matches the test name)
cargo test test_name_substring

# Run tests for one workspace crate only
cargo test -p domain_types

# Apply pending migrations to DATABASE_URL (required before `cargo build`/`cargo check`
# if the DB is behind — see note below)
cargo sqlx migrate run

# Regenerate sqlx offline query cache
cargo sqlx prepare -- --tests

# Start via Docker Compose
docker-compose up
```

### Adding a new environment variable

**Reading the variable in `config/` is only the first of six places.** A variable that works locally
and is silently missing in production has been shipped more than once, because the container passes
through an explicit list. Every new variable goes into **all** of these:

1. `src/config/` — where it is read;
2. `.env.example` — commented out, with both the `localhost` and the in-Docker form when the value
   is a host;
3. `docker-compose.yml` — the `environment:` list of the `DickGrowerBot` service. **Missing it here
   means the variable never reaches the container**, no matter what `.env` says;
4. `Dockerfile` — the `ARG` list at the bottom. It changes nothing at runtime (`ARG` is build-time
   only), but the list is kept complete as the inventory of what the image understands;
5. `README.md` and this file — what it does and what happens when it is unset;
6. the server-configs repo — `DickGrowerBot/docker-compose.yml` (the same `environment:` list) and
   `DickGrowerBot/.env.sops` (the value itself, through `make secret-edit`).

### Required environment variables (`.env`)

```
DATABASE_URL=postgres://...
TELOXIDE_TOKEN=...
```

### Optional: bot HTTP-client timeouts

The bot's Telegram API client (`config/bot.rs`, `BotConfig::build_bot`) has tunable timeouts so a
stalled request (e.g. when DPI equipment lets the connection hang instead of resetting it) fails
after a bounded time instead of blocking update processing. Both vars are optional and each
overrides only its own knob; leaving **both** unset keeps teloxide's stock client:

```
BOT_HTTP_CONNECT_TIMEOUT_SECS=5  # teloxide default when unset
BOT_HTTP_TIMEOUT_SECS=17         # total per-request timeout; teloxide default when unset
```

Standard proxy env vars (`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY`) are auto-detected by
reqwest and honored either way. `TELOXIDE_PROXY` is a teloxide-specific var read only by the stock
`Bot::from_env()` client (i.e. when both timeouts are unset).

### Observability / tracing

Logging and tracing go through `tracing` (initialized in `src/observability.rs` via
`observability::init_tracing()` in `main.rs`). The bot logs with `tracing::{info,warn,error,debug}!`
only; the `log::*` records of the libraries (teloxide, sqlx, reqwest) are captured by the
`tracing-log` bridge, so everything shares one pipeline.

```
RUST_LOG=info                                      # verbosity of the console and of the export
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317  # spans, OTLP/gRPC; unset => spans are not exported
OTEL_EXPORTER_OTLP_LOGS_ENDPOINT=http://localhost:9428/insert/opentelemetry/v1/logs  # log records, OTLP/HTTP
```

The console layer is **always** on and is the fallback: `docker logs` and journald keep working, and
it is what remains when the collector can't be reached. The two signals need two variables because
they go to different places and speak different protocols — the spans to Jaeger/Tempo over gRPC, the
records to VictoriaLogs over HTTP (the full URL, path included). The logs endpoint is always passed
to the exporter explicitly; left to itself it would fall back to the traces one and post log records
to the tracing backend.

Spans are exported over OTLP/gRPC (batch) when `OTEL_EXPORTER_OTLP_ENDPOINT` is set; the
service name is the crate name (`dick-grower-bot`). A trace of an update is rooted at the handler
that processes it; outbound user-service calls (tonic) are auto-instrumented, and W3C trace-context
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

### Optional: user-service integration

The bot can integrate with the [user-service](https://github.com/Kozalo-Blog/user-service)
microservice (gRPC) to read/update a user's preferred language across all of Kozalo's bots:

```
GRPC_ADDR_USER_SERVICE=host:port   # unset => integration disabled, personal /language hidden in PMs
USER_CACHE_TIME_SECS=360           # optional cache TTL for fetched users
```

`/language` is overloaded: in a private chat it changes the caller's personal language (via
user-service, above); in a group it sets a chat-wide language (admins only) that applies to
everyone and overrides each user's own preference. The chat-wide setting is stored in our own
`Chats.settings` (jsonb) column, so it works even when user-service is disabled:

```
CHAT_LANGUAGE_CACHE_TIME_SECS=3600 # optional TTL for the per-chat language cache (we own the data)
```

The proto contract is vendored as the `user-service-proto` git submodule and compiled by
`build.rs` (via `tonic-prost-build`), so **`protoc` must be installed** and the submodule
checked out to build:

```bash
git submodule update --init
```

Migrations run automatically on startup via `sqlx::migrate!` — but that's only at
runtime. `sqlx::query!`/`query_as!` macros type-check against the live schema at
`DATABASE_URL` when compiling (no `.sqlx/` cache, or it's stale), so **`cargo build`
and `cargo check` will fail with confusing type-mismatch errors if your local DB
hasn't had the latest migrations applied yet.** Run `cargo sqlx migrate run` first
whenever a build fails right after pulling migration changes. Requires `sqlx-cli`
(`cargo install sqlx-cli`).

## Architecture

### Workspace layout

| Crate | Purpose |
|---|---|
| `DickGrowerBot` (root) | Main application binary |
| `domain_types` | Shared domain primitive types and traits |
| `domain_types_macro` | Proc-macro crate — `#[domain_type]` derive |

### Layer breakdown

```
config/      — env-var config structs, feature flags
domain/      — pure domain types (primitives, objects, traits, errors)
handlers/    — teloxide update handlers and business logic
repo/        — sqlx repository impls; DB access only
help/        — help-message rendering (tinytemplate)
locales/     — rust-i18n translation files (YAML)
migrations/  — SQL migration files, auto-applied on startup (see DB Migrations below)
```

### Key frameworks

- **teloxide** (custom fork) — Telegram bot framework
- **sqlx** — async, compile-time checked SQL queries; offline cache in `.sqlx/`
- **tokio** — async runtime
- **axum** — HTTP server (webhooks / health)
- **rust-i18n** — i18n via `locales/` YAML files

### Domain type macro system

`#[domain_type]` (from `domain_types_macro`) generates newtype wrappers with arithmetic impls, `From`/`Into`, sqlx `Type`/`Encode`/`Decode`, and other trait impls from a simple attribute annotation. See `domain_types/src/traits.rs` and `domain_types_macro/src/lib.rs`.

### Feature-oriented handler/repo pairing

Each bot feature is a vertical slice: a file in `handlers/` (e.g. `dick.rs`, `pvp.rs`,
`loan.rs`, `promo.rs`, `perks.rs`, `dod.rs`, `import.rs`) driving business logic, backed
by a matching file in `repo/` (`dicks.rs`, `pvpstats.rs`, `loans.rs`, …) that owns the
SQL. When adding a feature, follow this pairing rather than mixing DB access into handlers.

### Dependency injection

Repositories are grouped in a `Repositories` struct and injected into handlers via the `deps!` macro. Handlers do not construct repos directly.

### Feature toggles

Runtime features are gated by environment variables parsed in `config/`. Check `config/` for the list of flags.

## Code Style

- **A log message is a constant; the values are fields.** Use `tracing::{debug,info,warn,error}!`
  (never `log::*`) and keep the message text free of interpolated values, so that repeated events
  group together in the log database. Messages are lower-case and have no trailing dots. Pass the
  error of a failed operation as an `error` field — `tracing-opentelemetry` turns it into an
  exception event on the span, which is how a failure becomes visible in the trace. Don't repeat
  what the span already carries: an instrumented function's `chat_id`/`uid`/`lang_code` are printed
  with every line anyway.

  ```rust
  // ❌ the values are baked into the text, the message is unique every time
  tracing::warn!("daily shrink: couldn't notify chat {chat_id}: {err:#}");

  // ✅ constant message, values as fields (chat_id comes from the span)
  tracing::warn!(error = format!("{err:#}"), "couldn't notify the chat about the shrinks");
  ```

  Use `%value` for `Display`, `?value` for `Debug`, and `format!("{e:#}")` for an `anyhow` error
  whose whole chain is worth keeping on one line.

- **A comment describes the code, never the change.** Write comments in short, plain English: what
  the code does, and why if that isn't obvious. Never mention a change, a diff, an issue number, or
  the previous version ("one statement instead of a transaction", "renamed from…"). The same goes
  for what is deliberately *absent*: removed code leaves no comment behind, so no "no X here on
  purpose", "X was removed because…", "bring X back if…". All of that belongs in the commit message.
  If the reasoning is worth keeping, put it in `CLAUDE.md` or `README.md`. When nothing non-obvious
  is left to say, write no comment at all.

  ```rust
  // ❌ only makes sense to someone reading the diff
  // No OpenTelemetry layer here on purpose. It used to trace the webhook, but those spans were
  // empty, so it was removed — bring `axum-tracing-opentelemetry` back if a real route appears.
  let app = axum::Router::new().merge(bot_router);

  // ✅ no comment; the reason lives in the "Observability / tracing" section above
  let app = axum::Router::new().merge(bot_router);
  ```

- **Prefer domain-type wrappers over raw primitives for long-living, meaningful values.** Config
  fields, struct fields, and public function parameters/returns that carry a domain concept (a count
  of days, a length, a ratio, an id, …) should use the newtype from `domain_types` / the
  `#[domain_type]` macro (e.g. `DaysCount`, `Length`, `Ratio`, `UserId`) rather than a bare `i32` /
  `u32` / `String`. This keeps units and intent in the type system and stays consistent with the
  repo layer, which already speaks domain types. Convert to the primitive only at the true boundary
  (e.g. `value() as i32` right before an sqlx bind). If a suitable wrapper doesn't exist yet, add one
  (see `domain_types/src/traits.rs` and `domain_types_macro/src/lib.rs`) instead of falling back to a
  primitive. Short-lived locals and loop indices don't need wrapping.

  ```rust
  // ❌ raw primitives for domain concepts on a long-living config struct
  pub shrink_grace_days: i32,
  pub shrink_events_days: u32,

  // ✅ domain-type wrappers
  pub shrink_grace_days: DaysCount,
  pub shrink_events_days: DaysCount,
  ```

- **ALWAYS** break a function signature onto one parameter per line when the single-line signature
  reaches **120+ characters**. Put the opening `(` at the end of the `fn` line, each parameter on
  its own line with a trailing comma, and the closing `)` plus return type on their own line
  (rustfmt block style); keep any `where` clause after the `)`:

  ```rust
  // ❌ too long on one line
  pub async fn set_chat_language(&self, chat_id: &ChatIdPartiality, lang: Option<SupportedLanguage>) -> anyhow::Result<()> {

  // ✅ one parameter per line
  pub async fn set_chat_language(
      &self,
      chat_id: &ChatIdPartiality,
      lang: Option<SupportedLanguage>,
  ) -> anyhow::Result<()> {
  ```

  Signatures under 120 characters may stay on a single line.

- **Avoid long, complex one-line expressions.** Break a method/`await` chain across lines at the
  dots, and don't inline a call inside an assertion: assign its result to a variable first, then
  assert on the variable. A trailing `.await.expect(...)` may stay together on one continuation line.

  ```rust
  // ❌ long chain inlined in the assertion
  assert_eq!(chats.get_chat_language(&kind).await.expect("couldn't read the language"), None);

  // ✅ split by dots, bind, then assert
  let lang = chats.get_chat_language(&kind)
      .await.expect("couldn't read the language");
  assert_eq!(lang, None);
  ```

- **Prefer combinators over `match` on `Result`/`Option`** when there are only two outcomes and
  you don't need `return`, extra conditions, or other special control flow. Use `map` /
  `map_err` / `and_then` / `unwrap_or_default` for the values and `inspect` / `inspect_err` for
  side effects (like logging) instead of spelling out `Ok`/`Err` (or `Some`/`None`) arms.

  ```rust
  // ❌ two-arm match just to log and fall back
  let file = match serde_saphyr::from_str(&content) {
      Ok(file) => file,
      Err(e) => {
          log::warn!("couldn't parse the file: {e}");
          Default::default()
      }
  };

  // ✅ inspect_err for the log, unwrap_or_default for the fallback
  let file = serde_saphyr::from_str(&content)
      .inspect_err(|e| log::warn!("couldn't parse the file: {e}"))
      .unwrap_or_default();
  ```

  A `match` is still the right tool when a branch needs `return`/`continue`, guards
  (`Err(e) if …`), or more than two outcomes.

## DB Migrations

Migration files live in `migrations/`, numbered sequentially. They are applied
automatically at *startup* (`sqlx::migrate!`) — no manual step needed to run the bot.

However, `cargo build`/`cargo check` compile-time-check queries against the live
`DATABASE_URL` schema (unless relying on the offline `.sqlx/` cache), so after adding
or pulling a new migration, apply it manually before building:

```bash
cargo sqlx migrate run
```
