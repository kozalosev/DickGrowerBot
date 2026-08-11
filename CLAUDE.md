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

### Optional: /support and user bans

```
SUPPORT_CHAT_ID=-1001234567890  # where /support relays messages; unset => command hidden and disabled
BAN_LIST_REFRESH_SECS=900       # how often the in-memory ban list is re-read from the DB
```

`Users.banned_until` (migration 34) holds the ban; `NULL` means the user is not banned. It is set
only from outside the bot, with the SQL functions of migration 35 (`erase_user`, `ban_user`,
`unban_user`) — there is no self-service delete command, and there must not be one: a deletion
request is answered by hand. `erase_user` deletes every row the user owns but **keeps** the `Users`
row (empty name, reset `created_at`, future `banned_until`), which is why none of the missing
`ON DELETE CASCADE` foreign keys matter.

Because the ban is written straight into the database, the bot polls for it: `bans::BanList` keeps
the whole (tiny) list in memory, refreshes it on a timer and on SIGHUP.

That list is only how fast a banned user sees the polite message — **the enforcement is migration
36**, a `BEFORE UPDATE` trigger on `Users` that refuses any statement touching a banned user's row
unless the statement changes `banned_until` itself (which is how the three admin functions pass).
It sits on `Users` rather than `Dicks` for three reasons: the check is free there (`banned_until` is
on the row being updated, so there is nothing to look up), `Users` has exactly one production write
(`create_or_update`, `src/repo/users.rs:13`) and nothing bulk, and a trigger on `Dicks` would fire
during the chat merge's bulk insert (`src/repo/chats.rs:609`) and abort the whole merge. The error
carries SQLSTATE `GD3E1` (`handlers::BANNED_SQL_CODE`) with the ban's end date as its message, which
both call sites turn back into the `errors.banned` notice.

`checks::reject_banned_users()` sits in the middle of the dispatcher tree in `main.rs`. The order
there is load-bearing: **nothing above the gate may write a row for its sender.** Only `/help`,
`/privacy` and `/support` qualify. `/start` does not — a promo deeplink activates a code — and
neither does `/language`, which writes to user-service. The gate is an `Update`-level filter rather
than a `Message` one because the inline handler upserts a `Users` row too, and that upsert would
restore an erased name.

### Optional: restricting the bot to forum topics

A forum's admins can confine the bot to chosen topics with `/topics` (issue #102), which answers
with an inline picker: allow or forbid the topic it was invoked in, or allow every topic again. The
allowlist lives in the same `Chats.settings` jsonb as the chat language, under `topics` — an
id-keyed set, `{"<topic id>": true}`, absent or empty meaning "every topic", which is the default.
Only the keys carry meaning; an object is used rather than an array because it merges and deletes
in one statement and dedupes for free.

```
CHAT_TOPICS_CACHE_TIME_SECS=3600   # optional TTL for the per-chat allowed-topics cache
```

`checks::reject_forbidden_topic()` sits at the very top of the dispatcher tree, above even the ban
gate, so **every** command is covered rather than the game ones only. Its twin,
`reject_forbidden_topic_callback()`, does the same for the buttons, above the other callback
branches: a keyboard outlives the message it came with, so without it the whole game could still be
played from a forbidden topic by tapping an older message. They may sit there because they
write nothing for the sender — the property the ban gate's ordering depends on.

`/topics` is registered *above* the gate, so a chat can't lock itself out of its own setting. That
placement is the whole exemption: the gate needs no special case for it, and the branch matches
every form of the command (`@username` suffix included) because `filter_command` knows the bot's
real name. Moving the branch below the gate would silently break this — `checks::test` pins it
down.

Four things keep the gate narrow: only real forums (`utils::is_forum` — a supergroup linked to a
channel puts a `message_thread_id` on discussion-thread messages too); only commands, not every
message (a notice on each one would be noisier than the bot the setting was meant to quiet); only
**our** commands, matched by name against `commands::COMMAND_NAMES` and by the `@username` Telegram's
menu appends — a group usually holds several bots, and answering for another one's command is the
noise this feature exists to remove; and fail-open on a database error.

Two limits are Telegram's, not ours, and both shape the design:

* **Topics have no names here.** There is no `getForumTopics`, and a name only ever arrives on the
  service message of a topic being created. Nothing is stored or shown for them: a list of `#42`
  labels says less than a count. So the picker speaks only about the topic it was opened in —
  whether the bot works there, plus how many topics it is confined to overall — and every topic is
  allowed or forbidden from inside itself. That is also why there is no "drop that other topic"
  button: it could not be labeled.
* **Inline mode can't be restricted.** An `InlineQuery` carries no thread, and the
  `inline_message_id` decoded in `handlers/utils/tghack.rs` holds only
  `(dc_id, peer, message_id, access_hash)` — a message's topic isn't derivable from its own id.
  That half is issue #76.

Because the daily-shrink broadcast replies to nothing, it names the topic outright
(`AllowedTopics::primary()`, the lowest allowed id — a jsonb object has no order to recover).
That also fixes a failure that predates the feature: a forum whose General topic is closed refuses
a message sent without a topic. The setup message needs none of this — it only ever goes to legacy
basic groups, which can't be forums.

### Optional: self-destruction of messages

A busy chat drowns in the bot's own answers, so each of them may be given a lifetime and removed
when it runs out (issue #49). Messages fall into four groups — `Notice` (help, privacy, statuses),
`Report` (`/top`, `/stats`), `Event` (growths, DoDs, fought battles) and `Application` (offers
waiting for an answer) — and each group has a delay of its own, zero meaning permanent. Private
chats are never cleaned up; they aren't noisy.

```
MSG_SELFDESTRUCT_DELAY_NOTICE=2         # minutes; 0 or unset => the group is permanent
MSG_SELFDESTRUCT_DELAY_REPORT=5
MSG_SELFDESTRUCT_DELAY_EVENT=0          # the chat's history — permanent by default
MSG_SELFDESTRUCT_DELAY_APPLICATION=60
MSG_SELFDESTRUCT_READING_SPEED_CPM=500  # a long message lives at least as long as it takes to read
MSG_SELFDESTRUCT_WARNING_SECONDS=15     # grace period showing "will be deleted in N seconds"
MSG_SELFDESTRUCT_MODE=WITHOUT_COMMAND   # DISABLED | ENABLED | ONLY_WITH_COMMAND | WITHOUT_COMMAND
MSG_SELFDESTRUCT_POLL_SECS=5            # how often the worker looks for the due messages
MSG_SELFDESTRUCT_BATCH_SIZE=50          # messages one run takes on
MSG_SELFDESTRUCT_CONCURRENCY=8          # how many of them it acts on at once
MSG_SELFDESTRUCT_LEASE_SECS=300         # how long a claimed batch is held out of reach
MSG_SELFDESTRUCT_INLINE_GROUPS=         # comma-separated groups; empty => inline messages are kept
MSG_SELFDESTRUCT_RETRY_DELAY_SECONDS=60 # the first wait after a failure; it doubles with each one
MSG_SELFDESTRUCT_MAX_RETRY_DELAY_SECS=3600  # the cap that doubling stops at
MSG_SELFDESTRUCT_MAX_ATTEMPTS=3         # attempts before the row is marked `failed` and left alone
MSG_SELFDESTRUCT_TABLE_CLEANING_DELAY=1440  # minutes a finished row is kept; 0 => for ever
```

**Everything goes through the database**, short-lived groups included: `SelfDestructionService`
(`handlers/utils/self_destruction.rs`) only writes rows into `Scheduled_Message_Deletions`
(migration 37), and the worker in `scheduler/deletions.rs` claims what is due and acts on it. The
claim *leases* its batch — one `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED) RETURNING`
that pushes `fire_after` `MSG_SELFDESTRUCT_LEASE_SECS` out. The lease, not the lock, is what makes the claim
exclusive: the row's lock lives only as long as that statement, while the requests it leads to take
much longer. A worker killed mid-batch leaves its messages to be claimed again once the lease runs
out. That is what a restart between an answer and its
deletion costs nothing, and what makes an `Application` delay of hours possible at all. The column
is `fire_after`, not `fire_at`: the worker polls, so it acts somewhat after the moment stored.

Two things are load-bearing in the schema:

* the **unique indexes** on `(chat_id, message_id)` and on `inline_message_id` make scheduling
  idempotent (`ON CONFLICT DO NOTHING`), which is why paging through a leaderboard can't push its
  deletion off for ever, and give `cancel` an exact key;
* the **`state`** enum carries the grace period in the same row: the message is edited into the
  warning and rescheduled, rather than held in memory. An inline row stays `created` to its end.

**Whether the bot may delete the command** is not in the schema at all — it lives in the cache
(see "The cache" below), because the bot is *told* the answer rather than having to ask for it.
`handlers::rights` writes it from every `my_chat_member` update, which Telegram sends when the bot
is added, promoted or demoted, and `scheduler::deletions` writes `false` when a deletion is refused.

Where the cache knows nothing, what happens depends on the mode, and the two differ because a wrong
guess costs them different things:

* `ENABLED` guesses "yes" and finds out by trying. A refusal costs one request, marks the row
  `failed` and teaches the cache. Nobody sees it, so **it never asks Telegram**.
* `ONLY_WITH_COMMAND` can't guess. The answer is deleted *before* its command, so a refusal would
  leave the command sitting alone in the chat — the very thing the mode exists to prevent. This is
  the only place `getChatMember` is still called, and only when nothing is known.

That asymmetry is the whole reason the check survived at all: for `ENABLED` it would be pure
overhead.

**What becomes of a row** is the whole state machine, and **no ending deletes it**:

| Ending | State |
|---|---|
| the message was deleted or replaced | `removed` |
| it was already gone (someone got there first) | `removed_before` |
| it outlived Telegram's 48 hours while it waited | `expired` |
| the bot may not touch it, or the chat is gone | `failed` |
| every attempt failed for a reason that looked transient | `failed`, after `MAX_ATTEMPTS` |
| anything else | unchanged; postponed by the back-off, `attempts` + 1 |

**The counter counts endings, one per message.** `self_destruction_total{group,kind,outcome}` grows
only when a row reaches a terminal state, and the `outcome` values are those states. So it shows the
same four things as `self_destruction_finished{state}`, but as a rate instead of a current number.
`scheduler::deletions::finish` writes the row and the counter together, so they can't say different
things.

A warning is not an ending, so it is not counted: the message will be counted later, when it goes.
A retry is not an ending either, and it has its own counter,
`self_destruction_retries_total{group,kind}`. This is what keeps the outcomes equal to the number of
messages. A message that was retried twice and then deleted is one `removed` plus two retries, not
two failures and one success.

A finished row is stamped with `finished_at` and left where it is, so
`Scheduled_Message_Deletions` is the whole account of what the worker did — including the successes,
which is what makes a failure rate readable from the table itself rather than only from Prometheus.
`removed_before` is kept apart from `removed` on purpose: a chat where it is the usual ending
already has a human or another bot doing the cleaning, and the delays configured here are only
getting in their way.
`SELECT state, count(*) … WHERE finished_at IS NOT NULL` is the first thing to look at when messages
stop disappearing; the same numbers are exported as `self_destruction_finished{state}`.
`scheduler::spawn_deletion_cleaner` deletes them `MSG_SELFDESTRUCT_TABLE_CLEANING_DELAY` minutes
later — a task of its own, because clearing the history must never be part of the run that wrote it.
**A retention of 0 keeps everything for ever**: right while debugging the worker, unbounded growth
on a busy bot.

The wait between attempts is **exponential** — `MSG_SELFDESTRUCT_RETRY_DELAY_SECONDS` doubled once
per failure already recorded, capped at `MSG_SELFDESTRUCT_MAX_RETRY_DELAY_SECS`
(`scheduler::deletions::backoff`). The count comes from the claimed row's `attempts`, not from the
`postpone` that follows: the delay has to be known before it is written. Keep the cap well under
Telegram's 48 hours. Without a cap the doubling would push an old row past that limit, and every
attempt after it is refused for sure.

**Only `MAX_AGE` stays a constant** (`scheduler::deletions`). 48 hours is Telegram's number, and a
setting for it would be a knob that changes nothing. Everything else the worker uses — the batch
size and the lease too — is an environment variable, because the right value depends on how busy
the bot is, and finding it should not need a rebuild.

**Deciding on `MSG_SELFDESTRUCT_POLL_SECS`** takes two metrics, not one, because a long tick has two
opposite causes. `run_pending_deletions` carries `#[autometrics]`, so how long a run took is
`function_calls_duration_seconds{function="run_pending_deletions"}` — and the default buckets
include `5.0`, the stock interval, so "the share of runs that fit into one tick" needs no
interpolation. How much a run had to do is `self_destruction_batch_size`, bucketed at the default
batch limit. Read together:

| duration | batch size | what it means |
|---|---|---|
| under the interval | any | the queue keeps up; leave it alone |
| at or over it | hitting the limit | saturated — raise `MSG_SELFDESTRUCT_CONCURRENCY`, and the batch size with it if a run then empties the queue early; a longer interval makes it worse |
| at or over it | well under it | Telegram is slow, not the queue — a longer interval costs nothing |

**The knob for throughput is `MSG_SELFDESTRUCT_CONCURRENCY`, not the batch size.** A run gets
through that many messages per round trip to Telegram; the batch size only bounds how much it
claims, so raising it alone lengthens the run and drains the queue no faster. Keep the concurrency
under `DATABASE_MAX_CONNECTIONS` — every finished message writes a row — and watch
`telegram_request_errors_total{kind="rate_limited"}` after raising it.

Changing `MSG_SELFDESTRUCT_BATCH_SIZE` needs no other change. The buckets go up to 500, well past
any sane limit, and the limit itself is exported as `self_destruction_batch_limit`
(`spawn_deletion_worker` sets it), so the graph draws the line to compare against instead of
holding a copy of the number.

The empty runs are measured too: an idle worker is what tells a queue that keeps up from one that is
only being asked for less than it holds. `self_destruction_total` alone won't stand in for the batch
size — a warned message isn't counted there, so the messages-per-run ratio comes out low. Neither
will `TASK_SELF_DESTRUCTION`: a `TaskMonitor` sums the poll and idle time of one task, and the worker
is a single endless loop, so every tick blends into the same number.

The age is checked **before** the request, against `created_at` (the row is written right after the
message is sent, so it is the message's age). Delays are capped below 48 hours, so only a queue that
fell behind — a long outage, a chain of retries — can produce a message that old, and spending a
request on a refusal that is certain teaches nothing.

The command behind an answer is a row of its own (`message_kind = 'command'`), scheduled at
`fire_after + warning` so that both messages disappear together — a user's message can't be edited
into the warning the answer shows meanwhile. `ONLY_WITH_COMMAND` writes *neither* row when the bot
may not delete the command: the point of that mode is that a lone answer is worse than both staying.

Two limits are Telegram's:

* **A bot can't delete a message older than 48 hours.** Two constants come out of that one limit,
  an hour apart. Every delay is cut down to **47** (`config::MAX_DELAY`), reading-time stretch
  included: the hour of headroom pays for the poll interval, the lease, the warning's grace period
  and the waits between failed attempts, so a message scheduled at the cap is still deletable when
  its request finally goes out. **48** is the real thing (`scheduler::deletions::MAX_AGE`): a
  message that reaches it — only a queue that fell behind can bring one there — is marked `expired`
  without spending a request on a certain refusal.
* **An inline message can never be deleted, only edited.** So those are replaced with the
  `self_destruction.placeholder` text instead, and only for the groups
  `MSG_SELFDESTRUCT_INLINE_GROUPS` names — whatever is put there stays in the chat for good, which
  is worth being conservative about. The bot gets an `inline_message_id` from a
  `ChosenInlineResult`, so that is where the scheduling sits. Legacy groups send no chosen result,
  so `inline_callback_handler` schedules too. The second call only fills the gap: the unique index
  on `inline_message_id` makes the insert do nothing for a message that already waits, so paging
  through a leaderboard does not delay its placeholder.

An application answered before it expires is *cancelled* (`loan.rs`, `pvp.rs`): the message has
stopped being an offer, and its outcome is kept like any other event.

### One throttle for the schedulers

The daily shrink and the deletion worker both reach many chats at once, and both hold
`teloxide`'s `Throttle`. They share **one**, built by `scheduler::throttled` and cloned into both
(`main.rs`).

**It only governs the shrink.** The adaptor throttles the message-*sending* methods and passes
everything else through, so the deletion worker — which only deletes and edits — is not bounded by
it at all. That is teloxide's judgement, not an oversight: the documented limits (30 a second, one
a second per chat) are about sending, and a deletion produces no message and no notification. What
bounds the worker is `MSG_SELFDESTRUCT_CONCURRENCY`. If that judgement is ever wrong, a 429 is not
final — the row is postponed and tried again, and it shows as
`telegram_request_errors_total{kind="rate_limited"}`.

Sharing is not a nicety. The adaptor counts requests inside a worker task that the wrapper spawns,
and a clone only adds a handle to that same worker. Two wrappers would be two workers with two
separate histories, so each would allow the full 30 requests per second and 1 per second per chat.
Together they would send twice as much, and a pause after a 429 would stop only the one that got it.

The dispatcher and the handlers still use the plain `Bot`. They answer one user at a time, and their
rate follows the users.

Because those answers are **not** counted by the throttle, the schedulers are given less than
Telegram allows. What is left over is the room the answers need.

```
THROTTLE_MESSAGES_PER_SEC_OVERALL=20      # Telegram allows 30; the rest is for the answers to users
THROTTLE_MESSAGES_PER_SEC_CHAT=1
THROTTLE_MESSAGES_PER_MIN_CHAT=15         # private chats and legacy groups
THROTTLE_MESSAGES_PER_MIN_SUPERGROUP=10   # every chat whose id starts with -100
```

The last one is the interesting one, and its teloxide name
(`messages_per_min_channel_or_supergroup`) is easy to misread. The adaptor picks it whenever
`ChatId::is_channel_or_supergroup()` holds, and that is only a check that the id starts with -100.
Supergroups pass it, so this limit governs most of the chats the bot serves, while
`THROTTLE_MESSAGES_PER_MIN_CHAT` is left for private chats and the few legacy groups. The bot is
never in a channel; the name is teloxide's.

`Settings::on_queue_full` is wired to `telegram_throttle_queue_full_total`. The queue holds 30
requests, and teloxide reports it as full at most once every 4 seconds, so the counter counts
moments, not requests. A few of them at midnight are normal — that is the shrink broadcast. A number
that keeps growing all day means the schedulers ask for more than Telegram allows.

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

### Observing the Telegram Bot API calls

The outgoing requests are watched by `TelegramObserver` (`src/telegram_observer.rs`), attached to
the bot in `config/bot.rs`. It implements `RequestObserver`, a hook **our teloxide fork** adds to
`Bot` (branch `feature/request-observer`, `crates/teloxide-core/src/observer.rs`) — the `teloxide`
dependency in `Cargo.toml` points at that branch, and the hook is meant to be contributed upstream.

Each request produces three things:

* `telegram_request_duration_seconds{method,outcome}` — how long the call took. The failed calls
  are measured too, so a timeout (the slowest case there is) is in the histogram rather than
  missing from it. `outcome` is `ok` or one of the kinds `telegram_request_errors_total` uses; both
  come from the same `error_handler::classify`, so the two metrics always agree.
* a `telegram_request` client span, so a slow API call is a child span of the handler's trace
  instead of an unattributed gap inside it.
* when the API answers with `ApiError::Unknown` — its way of saying it disliked the payload without
  saying which part — an `error` record carrying the serialized request body. That is the only way
  to find out which entity or which over-long text was rejected.

The observer sits *below* the adaptors, so the time `Throttle` holds a request back is not counted
as request time. A multipart request (the file-uploading methods) is a stream by then and has no
body to log; it is still measured.

Changing any of this means updating the Grafana dashboard in the server-configs repo, next to the
"Telegram API request errors by kind" panel.

### Optional: the cache

Short-lived values live in Redis (`src/cache.rs`), which is the one place that keeps them.

```
REDIS_HOST=localhost    # unset => the cache is off
REDIS_PORT=6379
REDIS_PASSWORD=…
REDIS_CACHE_TTL_SECS=21600
```

**Nothing here is a source of truth.** A miss is answered by the caller doing the work again, so a
server that is down, slow or simply absent costs only the work it would have saved. Every failure is
logged and swallowed: `Cache` returns no errors and never panics, and an unreachable server at
startup leaves `Cache::Disabled` rather than stopping the bot. That is also why `REDIS_HOST` is
merely a switch — **the bot must start and run without it**, which is the path most likely to rot
and so has a test of its own.

The client is `redis` with `ConnectionManager`, and there is deliberately **no pool**. Redis runs
commands one at a time, so extra connections buy no parallelism; the protocol multiplexes instead,
letting many requests share one socket, and `ConnectionManager` adds reconnection on top. A pool
would only add an acquire step that can fail, a size to choose, and a dead connection to retry.
Keep it this way unless something needs a connection to itself — `BLPOP`, `SUBSCRIBE`, `WATCH` —
and note that `MULTI`/`EXEC` is unsafe on a multiplexed connection, so an atomic operation wants a
single command (`SET NX EX`) or a Lua script.

The container in `docker-compose.yml` runs **Valkey**, the BSD-licensed fork. The protocol is Redis,
which is what the service, the network and the variables are named after; only the binaries differ
(`valkey-server`, `valkey-cli`). It sits behind the `redis` Compose profile, like `user-service` and
`tracing`.

Everything else the bot caches is still in process memory, each with its own TTL and mutex —
`topics.rs`, the two in `users/mod.rs`, `bans.rs`, and the PVP locks in `handlers/utils/locks.rs`.
Moving them here is tracked separately. The locks are the interesting one: an in-memory `HashSet`
locks nothing across two instances, so that one is a bug rather than a tidy-up.

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

- **`new` builds a domain value; `literal!` is for the validated types only.** Four types validate
  anything: `Ratio`, `Percentage`, `FloatPercentage` (`ratio.rs`) and `PromoCode` (`promo.rs`).
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
  reasoning is worth keeping, it goes in this file or in the commit message.

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

  **Where the sink is ours, the conversion belongs to it, not to the caller.** `Gauge::set` and
  `Histogram::observe` (`src/metrics.rs`) take `impl SaturatingInto<i64>` / `impl ApproxInto<f64>`,
  so a caller hands over the domain value itself. The repo layer has done this all along: a query
  binds `uid as UserId` and sqlx's `Encode` does the converting.

  A cast that is genuinely right keeps an `#[allow]` carrying the reason, on the narrowest scope
  that works — never a whole function, or it will also cover the next cast written on that line.

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

## Tests against the database

The whole test binary shares **one** Postgres container, and every test takes a **database of its
own** out of it:

```rust
let db = fresh_db().await;
```

That is the entire API (`src/repo/test/mod.rs`). Three things make it work, and each of them is
load-bearing:

* **A runtime of its own.** Every `#[tokio::test]` builds a runtime and tears it down when the test
  ends, and a sqlx pool dies with the runtime that created it — sharing a pool between tests fails
  with *"a Tokio 1.x context was found, but it is being shutdown"* as soon as the first test
  finishes. So the container and its maintenance pool live on the runtime in
  `src/test_containers.rs`, reached with `test_containers::spawn(...)`. Not `block_on`, which panics
  inside a runtime. The per-test pool is built on the test's own runtime and dies with it, which is
  fine.
* **A template database.** The migrations run once per run into `test_template`; each test's
  database is `CREATE DATABASE … TEMPLATE test_template`, which is far cheaper than replaying every
  migration 54 times.
* **A reused container.** It is marked `ReuseDirective::Always` and deliberately outlives the run,
  so the next run finds it instead of paying the startup again. It is *only* removed by
  `task test:clean`. Its databases are named `test_run<pid>_<n>` and the ones left by earlier runs
  are dropped at startup — one test binary at a time is assumed, which is how `cargo test` runs.

**Every shared container is one `SharedContainer`** (`src/test_containers.rs`), declared as a
`static` next to the tests that need it — Postgres, the cache and VictoriaLogs. A reusable container
is matched by its **labels**, which is why the service name is a constructor argument and has to
differ: labelled alike, the second request is handed the first container and fails on a port that
isn't there. `task test:clean` matches the label by key, so it sweeps every value.

This replaced one container per test: ~35s for the suite instead of ~75s. Don't reintroduce a
per-test or per-file container, and don't put the shared pool in a plain `static` without the
runtime — both were tried and both fail in the ways described above.

## DB Migrations

Migration files live in `migrations/`, numbered sequentially. They are applied
automatically at *startup* (`sqlx::migrate!`) — no manual step needed to run the bot.

However, `cargo build`/`cargo check` compile-time-check queries against the live
`DATABASE_URL` schema (unless relying on the offline `.sqlx/` cache), so after adding
or pulling a new migration, apply it manually before building:

```bash
cargo sqlx migrate run
```
