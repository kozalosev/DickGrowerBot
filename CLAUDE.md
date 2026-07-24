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
