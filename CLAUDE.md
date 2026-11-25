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

# Remove the containers an interrupted test run left behind
task test:clean

# Run a single test (substring-matches the test name)
cargo test test_name_substring

# Run tests for one workspace crate only
cargo test -p domain_types

# Apply pending migrations to DATABASE_URL (required before `cargo build`/`cargo check`
# if the DB is behind — see note below)
cargo sqlx migrate run

# Before committing — all three, and `build` is not optional
cargo build && cargo clippy --tests && cargo test

# Regenerate sqlx offline query cache
cargo sqlx prepare -- --tests

# Start via Docker Compose
docker-compose up
```

### Why `cargo build` is in that list

`cargo check --tests`, `cargo clippy --tests` and `cargo test` all enable `[dev-dependencies]`. A
crate declared only there but used in shipped code therefore compiles under every one of them and
fails on the plain build — which is what the `Dockerfile` runs. `serde_json` did exactly this: it
was a dev-dependency, `cache.rs` and `dialogue.rs` used it in production code, and the release
binary had not compiled for several commits while every check passed.

The same gap hides dead code, since an unused function only warns on the non-test build. A helper
that exists for the tests is marked `#[cfg(test)]` rather than left `pub` — `Cache::get_json` and
`CacheConfig::redis` are the two of those.

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
5. the topic's file in `.claude/rules/` (see below) — what it does and what happens when it is
   unset. **Not `README.md`**: it is the
   project's index, and a variable's name, default or tuning advice is not what belongs there;
6. the server-configs repo — the line in `DickGrowerBot/.env`, committed in plain text there; a secret goes into
   `DickGrowerBot/.env.secrets.sops` instead, which only the user edits (`make secret-edit DIR=DickGrowerBot`), so
   give them the line. **`.env` lists every variable the bot reads**, a default value
   included; one whose mere presence changes something (a `DISABLE_*` switch) goes in commented
   out. Its compose file takes the whole `.env` through `env_file`, so it needs no change.

### Required environment variables (`.env`)

```
DATABASE_URL=postgres://...
TELOXIDE_TOKEN=...
```

### Building against the schema and the proto

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

## Topics

How each subsystem works, and why, lives in `.claude/rules/`, one file per topic. Each file is
loaded only when a file matching its `paths:` is read, so this one stays small. **Read the topic
before changing its code**, even when it didn't load on its own (when only creating a file, say),
and update it in the same commit as the code it describes.

A new topic is a new file with a `paths:` header naming the code it describes. A topic that
applies everywhere belongs here instead. Keep cross-references by file name (`see cache.md`),
since "below" means nothing across files.

| Topic | File |
|---|---|
| The bot's HTTP client and its timeouts | `bot-http-client.md` |
| Picking the Dick of the Day; one per chat per day | `dick-of-the-day.md` |
| What counts as a growth; perks and `Perk_States` | `growth-and-perks.md` |
| What the help mentions | `help.md` |
| Commands that only inform (`/voucher`, `/buy`, …), aliases, menus | `info-commands.md` |
| Checking an inline PvP offer | `pvp.md` |
| `/support`, its replies, user bans | `support-and-bans.md` |
| Rate limiting, `/start` spam, commands that arrive too late | `rate-limiting.md` |
| How many guesses a promo code takes | `promo.md` |
| How a command is counted (`usage_total`, rejections) | `command-metrics.md` |
| What each signal means | `signals.md` |
| How a span of time is written in a setting | `durations.md` |
| The pool's acquire timeout, `statement_timeout` | `database-pool.md` |
| The bot's usernames; how a player's name is shown | `bot-names-and-usernames.md` |
| Restricting the bot to forum topics | `topics.md` |
| Self-destruction of messages and its worker | `self-destruction.md` |
| The daily shrink and its broadcast queue | `daily-shrink.md` |
| The one `Throttle` | `throttle.md` |
| Tracing, logs, errors, panics | `observability.md` |
| Telegram API observer, moot refusals, unexpected ids | `telegram-api.md` |
| The store of short-lived values (cache, locks), dialogue state | `cache.md` |
| user-service integration, languages | `user-service.md` |
| Tests against the database | `db-tests.md` |

## The production server

When a problem is about how the bot behaves in production (an error users see, a slow command, a broadcast that
didn't finish), look at the server before guessing from the code. `ssh server-claude <command>` runs one read-only
command there; the user set it up for exactly this. The bot's containers are `dickgrowerbot`,
`dickgrowerbot-postgresql` and `dickgrowerbot-redis`:

```bash
ssh server-claude docker ps                                       # what runs, since when, healthy or not
ssh server-claude docker logs --since 2h dickgrowerbot            # the bot's log (also --tail N, --until T)
ssh server-claude docker inspect dickgrowerbot                    # status, health, restarts, image (nothing else)
ssh server-claude docker stats                                    # CPU and memory now
ssh server-claude docker logs --since 2h dickgrowerbot-postgresql # Postgres: slow or cancelled statements, backups
ssh server-claude cat DickGrowerBot/.env                          # the settings in production (server-configs)
ssh server-claude git log --oneline -5                            # what server-configs runs
```

A gate on the server checks every command against a whitelist (the server-configs repo, `CLAUDE.md`, "Server
access for Claude", and `scripts/claude-gate.sh`): no shell, quotes, pipes or globs (`--since -1h`, not
`--since "1 hour ago"`), no `docker exec`, no database shell, no secrets. Filter the output here, not there. Anything
that changes the server (a restart, a setting, a query that writes) is for the user to run.

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

**A quantity that can't be negative takes an unsigned inner type**, not a validator: there is then
no invalid value to refuse, so the constructor and the arithmetic stay infallible. Only a *range* is
worth validating (`Ratio`, `Percentage`), because no integer type encodes one.

Postgres has no unsigned column, so the macro stores such a type in the signed integer of the same
width — `u16` in an `int2`, `u32` in an `int4`, `u64` in an `int8`. Only `u8` widens, as Postgres
has no one-byte integer. Encoding and decoding convert rather than cast, and can refuse; a value in
the half of the range that has no signed counterpart would not have fit the column either.

Two things follow at the call sites. Arithmetic **saturates** instead of returning an `Err`, so
`page - 1` on the first page is the first page. And `.value()` is unsigned, so a place that mixes it
with a signed number — `LengthChange::value`, a Prometheus gauge — casts at that point.

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

- **`new` builds a domain value; `literal!` is for the validated types only.** Six types validate
  anything: `Ratio`, `Percentage`, `FloatPercentage` (`ratio.rs`), `PromoCode` (`promo.rs`),
  `PerkName` (`perk.rs`) and `StartParameter` (`deeplink.rs`).
  They alone have the `check_literal`/`from_literal` pair, and
  `literal!(Ratio = 0.5)` is what makes the `assert!` run while the code is compiled; a bare
  `Ratio::from_literal(0.5)` skips the `const` block, and with it the check.

  Everything else takes an unsigned or plain inner type instead of a validator, so there is nothing
  to force: `new` is `const` and infallible and is all a constant needs.

  ```rust
  // ✅ validated: the assert runs during the build
  grow_shrink_ratio: literal!(Ratio = 0.5),
  code: literal!(PromoCode = "test10"),

  // ❌ validated, bare: no const block, so nothing is checked at all
  grow_shrink_ratio: Ratio::from_literal(0.5),

  // ✅ everything else: nothing to check, so no ceremony
  top_limit: Limit::new(10),
  ```

  **Strings validate too, and only the check is const.** `literal!` expands to
  `from_literal(const { check_literal(v) })`: the `const` block runs the validator, and
  `from_literal` allocates afterwards — a `String` can't exist in a `const` context, but a `&str`
  can be checked in one. So a string validator takes `&str` and serves both paths, the literals in
  the source and the values arriving from the database, the environment and Telegram. Working on
  `&str` in a `const fn` means working on bytes, since `chars()` isn't const; `validators.rs` has
  the helpers.

  **Nothing has to be remembered here** — the compiler picks for you. A validated `new` returns a
  `Result`, so it will not compile where the value itself is wanted; an unvalidated type has no
  `from_literal` to reach for. `clippy.toml` closes the last gap by forbidding the four bare
  constructors.

  That list has two silent failure modes: a path spelled wrong resolves to nothing and is ignored
  without a word, and it takes no globs, so a new validated type stays unprotected until it is
  added. `src/domain/primitives/literal.rs` guards it — `cargo test literal` compares the list
  against the types that declare a validator and fails if the two disagree.

  **Validate where a refusal means something.** A rule earns its place when it guards untrusted
  input and the caller can act on the answer, as `PromoCode` does for what a user types. A value
  that every call site would have to accept anyway — a display name, say — gains nothing from a
  fallible constructor and loses by it: the safe path becomes the lossy one, and the plain `new`
  turns into a panic waiting for someone to reach for it.

  The other thing a rule may guard is a **column that would refuse the value anyway**. `PerkName`
  takes 1 to 32 ASCII characters, and `Perks.name` is a `varchar(32)` with a `CHECK` saying the
  same — which is what makes reading a name back into the type unable to fail. Every name is a
  literal in our own source, so the check runs while the code is built and the `Result` is never
  seen at runtime.

  Where the failure shows up depends on the kind of constant: a named `const` item is evaluated by
  `cargo check`, while an inline `const` block — which is what `literal!` expands to — is evaluated
  during codegen, so only `cargo build` and `cargo test` report it. The IDE stays quiet.

- **A comment never points against the dependencies.** This one is strict. A module may describe
  what it is and what it depends on; it must **not** name, list or explain the things that depend on
  it. No `[`crate::handlers::…`]` in `cache.rs`, no "used by the daily shrink" in a repo method, no
  roll-call of call sites anywhere.

  Two reasons, and the first is enough. Such a comment is a second place to update when the caller
  moves, and nothing makes it fail — it rots silently while the code stays correct. The second: it
  is not the lower layer's business. `cache.rs` stores flags under keys; that a key happens to
  describe the bot's rights is knowledge it must not have, in code *or* in prose.

  ```rust
  // ❌ src/cache.rs — the arrow runs backwards
  //! A key is declared by whoever owns the value it names — `crate::handlers::rights` for the
  //! bot's rights in a chat.

  // ✅ the same file says only what is true of itself
  //! A key's shape is worth a type rather than a `format!` at each call site.
  ```

  The same restraint applies generally: prefer no comment to one that repeats what the code says,
  and never explain a design by describing what the alternative would have generated. If the
  reasoning is worth keeping, it goes in this file, in a topic of `.claude/rules/`, or in the
  commit message.

- **A file reads from its public surface downwards.** Constants and statics first, then the types,
  then the functions; within each group, what callers use comes before what only this file uses. A
  private helper — a small constructor, a one-line wrapper around a library call — belongs near the
  bottom, under the thing it serves.

  ```rust
  // ✅ src/cache.rs
  pub enum Cache { … }          // what the rest of the bot sees
  impl Cache { … }
  trait CacheKey: Display {}    // how it spells its keys
  struct BotAdminKey(…);
  async fn connect_to(…) { … }  // a helper of one call site
  #[cfg(test)] mod test { … }
  ```

  Not a rule to follow off a cliff: a type that only makes sense next to its user stays next to it,
  and a helper wanted in two places belongs between them. The point is that someone opening the
  file meets what it is for before how it manages.

- **A log message is a constant; the values are fields.** Use `tracing::{debug,info,warn,error}!`
  (never `log::*`) and keep the message text free of interpolated values, so that repeated events
  group together in the log database. Messages are lower-case and have no trailing dots. Pass the
  error of a failed operation as `error = flat!(e)` (`errors::flat`). Don't repeat what the span
  already carries: an instrumented function's `chat_id`/`uid`/`lang_code` are printed with every
  line anyway.

  ```rust
  // ❌ the values are baked into the text, the message is unique every time
  tracing::warn!("daily shrink: couldn't notify chat {chat_id}: {err:#}");

  // ✅ constant message, values as fields (chat_id comes from the span)
  tracing::warn!(error = flat!(err), "couldn't notify the chat about the shrinks");
  ```

  `flat!` records the error *as an error*, a `&dyn Error`, which a string can't be: the exporter
  writes it as `exception.message`, and `tracing-opentelemetry` adds an exception to the span. The
  exporter writes only the error's `Display` and never walks `source()`, so `flat!` first joins the
  whole cause chain into that `Display`, on one line — the log database stores a record per line.
  It takes any `Error`, and an `anyhow::Error` through its `Deref`.

  Use `%value` for `Display` and `?value` for `Debug` for everything that isn't an error.

- **A comment describes the code, never the change.** Write comments in short, plain English: what
  the code does, and why if that isn't obvious. Never mention a change, a diff, an issue number, or
  the previous version ("one statement instead of a transaction", "renamed from…"). The same goes
  for what is deliberately *absent*: removed code leaves no comment behind, so no "no X here on
  purpose", "X was removed because…", "bring X back if…". All of that belongs in the commit message.
  If the reasoning is worth keeping, put it in `CLAUDE.md`, a topic of `.claude/rules/` or
  `README.md`. When nothing non-obvious
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
  repo layer, which already speaks domain types. A query binds the wrapper itself with an `as`
  override (`uid as UserId`), which tells sqlx the type rather than converting anything; reach for
  `.value()` only where a plain number is genuinely wanted. If a suitable wrapper doesn't exist yet,
  add one (see `domain_types/src/traits.rs` and `domain_types_macro/src/lib.rs`) instead of falling
  back to a primitive. Short-lived locals and loop indices don't need wrapping.

  ```rust
  // ❌ raw primitives for domain concepts on a long-living config struct
  pub shrink_grace_days: i32,
  pub shrink_events_days: u32,

  // ✅ domain-type wrappers
  pub shrink_grace_days: DaysCount,
  pub shrink_events_days: DaysCount,
  ```

  A domain number answers `is_zero()`, so a comparison against zero is spelled with it rather than
  through `.value()`.

  **Booleans and `chrono` types stay bare.** A wrapper is worth it when the bare type doesn't say
  what it holds, so two values can be swapped unnoticed. A `bool` field is named at every
  construction, and `DateTime<Utc>` already carries its unit, epoch and timezone. A `NaiveDate` is
  named by its parameter, and no signature takes two dates of different meanings. Where two do meet,
  name them, as `AdjacentDates` does, before reaching for a type. Most uses are chrono arithmetic or a
  sqlx bind, so a wrapper would be unwrapped at nearly every call site.

  A bool that needs more than a name becomes an **enum**, not a newtype. That applies when two bools
  meet in one signature, or when the two states have names of their own (`DickOfDaySelectionMode`,
  `ChangeSource`). `Foo(true)` reads no better at a call site than `true`.

- **A number changing type says which conversion it is.** `as` is denied
  (`[workspace.lints.clippy]` in the root `Cargo.toml`), because one token means three different
  things and a reader can't tell them apart without knowing both types. Name it instead:

  | The conversion is | Use | A value that doesn't fit |
  |---|---|---|
  | exact | `From` / `Into` | can't happen |
  | out of range | `SaturatingInto` | stops at the nearer end |
  | not representable | `ApproxInto` | becomes the nearest that is |

  Both traits live in `domain_types::traits`, are implemented for the integer and float primitives
  there, and are generated for every numeric domain type by `#[domain_type]`. `SaturatingInto`
  covers integer to integer (where `as` wraps — this is the one that changes behaviour) and float to
  integer; `ApproxInto` covers integer to float and float to float. A float truncates toward zero,
  so a caller who wants rounding calls `.round()` first.

  ```rust
  // ❌ three different conversions, all spelled the same
  let pending = count.value() as i64;
  let ratio = config.loan_payout_ratio.value() as f32;
  let debt = value.min(i64::MAX as u64) as i64;

  // ✅ each one named
  let pending: i64 = count.saturating_into();
  let ratio: f32 = config.loan_payout_ratio.approx_into();
  let debt: i64 = value.saturating_into();
  ```

  **Where the sink is ours *and* the conversion is always the same, it belongs to the sink.** The
  repo layer does this: a query binds `uid as UserId` and sqlx's `Encode` does the converting.

  The metrics are the counter-example, and worth knowing before trying to tidy them. `Gauge::set`
  and `Histogram::observe` (`src/metrics.rs`) take a bare `i64` / `f64`, and the callers name the
  conversion. They cannot do otherwise: `domain_types` implements these traits **only for the pairs
  that lose something**, on the principle that an exact conversion must say `From`. So there is no
  `SaturatingInto<i64> for i64`, and a caller holding a Unix timestamp could not satisfy such a
  bound at all — while a caller holding a `Count` or a `usize` genuinely is narrowing and should
  say so.

  A cast that is genuinely right keeps an `#[allow]` carrying the reason, on the narrowest scope
  that works — never a whole function, or it will also cover the next cast written on that line.

- **Every map picks its hasher**, and std's `HashMap`/`HashSet` is not one of the choices
  (`disallowed-types` in `clippy.toml`):

  | The keys are | Use |
  |---|---|
  | ours, Telegram-assigned ids, enums, an operator's file | `FxHashMap` / `FxHashSet` (`rustc-hash`) |
  | chosen by a user — usernames, free text | `AHashMap` / `AHashSet` (`ahash`) |

  Fx is the fastest, but it is public and unseeded, so keys an attacker chooses can be made to
  collide and turn every lookup into a scan. AHash is seeded at random per process, which closes
  that, and is still faster than SipHash. The import's set of leaderboard names is the one place
  that needs it today.

  A library whose signature takes a std map (prometheus's `Desc`, the OTLP exporter's headers)
  gets one under an `#[allow(clippy::disallowed_types, reason = "…")]` on the statement that builds
  it. Generated code is handled the same way, on the module around it: `users::generated` for
  prost, and `translations` in `main.rs` for `rust_i18n::i18n!`, which is why that macro sits in a
  module of its own and is glob-imported back — an `#[allow]` on a macro call is ignored.

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

  **A diverging branch is not a reason to reach for `match`** — `let … else` covers it, and the
  handling goes in the `else`, which is the branch that already exists for it.

  ```rust
  // ❌ two arms only to log and bail out
  let expired = match repo.expire_stale(limit).await {
      Ok(expired) => expired,
      Err(e) => {
          tracing::warn!(error = format!("{e:#}"), "couldn't expire the rows");
          return
      },
  };

  // ✅ let-else, with the log in the branch that leaves
  let Ok(expired) = repo.expire_stale(limit).await else {
      tracing::warn!("couldn't expire the rows");
      return
  };
  ```

  `let … else` binds nothing in the `else`, so the error's own text is not available there and the
  message stands alone. That is the trade: where the text is worth more than the shape — an error
  whose chain is the only clue to what went wrong — bind it with a `match` instead.

  The `else` of a `let … else` must diverge. A fallback that produces a value instead of leaving is
  `if let … else` as an expression — which is also the only shape an `async` fallback can take, since
  `unwrap_or_else` takes no async closure.

  A `match` is still the right tool for guards (`Err(e) if …`) or more than two outcomes.

## DB Migrations

Migration files live in `migrations/`, numbered sequentially. They are applied
automatically at *startup* (`sqlx::migrate!`) — no manual step needed to run the bot.

However, `cargo build`/`cargo check` compile-time-check queries against the live
`DATABASE_URL` schema (unless relying on the offline `.sqlx/` cache), so after adding
or pulling a new migration, apply it manually before building:

```bash
cargo sqlx migrate run
```
