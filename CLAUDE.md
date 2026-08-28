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
BOT_HTTP_CONNECT_TIMEOUT=5s  # teloxide default when unset
BOT_HTTP_TIMEOUT=17s        # total per-request timeout; teloxide default when unset
```

Standard proxy env vars (`HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY`/`NO_PROXY`) are auto-detected by
reqwest and honored either way. `TELOXIDE_PROXY` is a teloxide-specific var read only by the stock
`Bot::from_env()` client (i.e. when both timeouts are unset).

### Optional: /support and user bans

```
SUPPORT_CHAT_ID=-1001234567890  # where /support relays messages; unset => command hidden and disabled
BAN_LIST_REFRESH=15m       # the backstop behind the notification, see below
```

`Users.banned_until` (migration 34) holds the ban; `NULL` means the user is not banned. It is set
only from outside the bot, with the SQL functions of migration 35 (`erase_user`, `ban_user`,
`unban_user`) — there is no self-service delete command, and there must not be one: a deletion
request is answered by hand. `erase_user` deletes every row the user owns but **keeps** the `Users`
row (empty name, reset `created_at`, future `banned_until`), which is why none of the missing
`ON DELETE CASCADE` foreign keys matter.

`bans::BanList` keeps the whole (tiny) list in memory, because `banned_until` is a **sync** function
in a dptree filter: every instance needs the list of its own whatever else exists, which is why this
one did not move into the shared store with the rest of #155. What it needed was not another place
to keep the list but a way to hear that it changed, and only the database can say so — nothing in
the bot ever writes a ban.

So migration 40 puts an `AFTER UPDATE OF banned_until` trigger on `Users` that notifies the `bans`
channel, and `spawn_listen_task` reloads on it through a `PgListener`. A trigger rather than a line
in each of the three admin functions: their bodies would have to be repeated in that migration and
kept in step for ever, and a ban applied by a plain `UPDATE` would still go unheard. `pg_notify`
is delivered on commit, so a rolled-back ban is never announced.

The timer stays behind it as the backstop — a notification sent while the listener is reconnecting
is heard by nobody — and so does SIGHUP. A listening connection can't serve queries and is held for
as long as it listens, so `establish_database_connection` builds the pool
`LISTENER_CONNECTIONS` larger and `DATABASE_MAX_CONNECTIONS` keeps meaning what an operator set it
to.

### How a span of time is written

Every setting that names one takes a number and a unit — `30s`, `15m`, `1h`, `3d`. A bare number is
seconds, so a value written before this still means what it did.

The unit is in the **value**, never in the name. A name that carried it (`BAN_LIST_REFRESH_SECONDS`)
had to be renamed to change the unit, and gave two places for the unit to be stated and so one for
them to disagree — `EnvDuration::minutes` on a `_SECONDS` variable compiled and was off by sixty.
`env_duration!` and `parse_duration` in `config/env.rs` are the whole of it, and `secs`/`mins`/
`hours`/`days` write the fallbacks beside them.

The one exception is `MSG_SELFDESTRUCT_DELAY_OPTIONS_MINUTES`, which keeps its suffix because it is
not a span: it is the list of minute counts `/cleanup` offers a chat, stored as minutes in the
`Chats.settings` jsonb and shown as minutes on the buttons. The same goes for the `*_DAYS` knobs
that are `DaysCount` — a count of days is what they mean, not a duration.

### How long a query waits for a connection

```
DATABASE_ACQUIRE_TIMEOUT=30s  # sqlx's own default; `.env.example` suggests 5s
```

A wait here is backpressure, not patience: queueing means the pool is empty, and waiting longer
creates no connections. The default matches sqlx so that reading the variable changes nothing by
itself; the shorter value is opted into, and is what keeps a handler's worst case comfortably inside
`PVP_LOCK_TIME`. Refusals under load mean `DATABASE_MAX_CONNECTIONS` is too small, not that
this is too short.

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
CHAT_TOPICS_CACHE_TIME=1h     # optional TTL for the per-chat allowed-topics cache
BOT_ADMIN_CACHE_TIME=1h       # optional TTL for what is known about the bot's rights in a chat
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
MSG_SELFDESTRUCT_DELAY_NOTICE=2m        # 0 or unset => the group is permanent
MSG_SELFDESTRUCT_DELAY_REPORT=5m
MSG_SELFDESTRUCT_DELAY_EVENT=0s         # the chat's history — permanent by default
MSG_SELFDESTRUCT_DELAY_APPLICATION=1h
MSG_SELFDESTRUCT_DELAY_OPTIONS_MINUTES=1,5,15,60,180  # the delays /cleanup offers a chat
MSG_SELFDESTRUCT_READING_SPEED_CPM=500  # a long message lives at least as long as it takes to read
MSG_SELFDESTRUCT_WARNING=15s    # grace period showing "will be deleted in N seconds"
MSG_SELFDESTRUCT_MODE=ENABLED           # DISABLED | ENABLED | ONLY_WITH_COMMAND | WITHOUT_COMMAND
MSG_SELFDESTRUCT_POLL=5s           # how often the worker looks for the due messages
MSG_SELFDESTRUCT_BATCH_SIZE=50          # messages one run takes on
MSG_SELFDESTRUCT_CONCURRENCY=8          # how many of them it acts on at once
MSG_SELFDESTRUCT_LEASE=5m          # how long a claimed batch is held out of reach
MSG_SELFDESTRUCT_INLINE_GROUPS=         # comma-separated groups; empty => inline messages are kept
MSG_SELFDESTRUCT_RETRY_DELAY=1m # the first wait after a failure; it doubles with each one
MSG_SELFDESTRUCT_MAX_RETRY_DELAY=1h    # the cap that doubling stops at
MSG_SELFDESTRUCT_MAX_ATTEMPTS=3         # attempts before the row is marked `failed` and left alone
MSG_SELFDESTRUCT_TABLE_CLEANING_DELAY=1d       # how long a finished row is kept; 0 => for ever
```

**Every chat may overrule all of that** with `/cleanup` (issue #128), an admins-only picker that
switches each of the four groups on or off for that chat alone. Only on and off: the mode stays the
operator's, because it is about the bot's rights rather than about taste.

```
CHAT_CLEANUP_CACHE_TIME=1h     # optional TTL for the per-chat cleanup-settings cache
```

The choices live in the same `Chats.settings` jsonb as the chat language and the allowed topics,
under `cleanup` — a group-keyed object of **minutes**, `{"notice": 5}`. A group is therefore in one
of three states, and all three are distinct: a number is the delay the chat chose, a zero is the
chat asking for that group to be kept, and an absent key is the chat leaving the decision to the
bot. The last two look alike until the operator changes his mind, which is exactly when they part.
`SelfDestructionConfig::delay_for_chat` is where the three meet.

The numbers on offer are `MSG_SELFDESTRUCT_DELAY_OPTIONS_MINUTES`, a sorted, deduped list parsed
into `DelayOptions`; "keep them" and "let the bot decide" are added by the picker itself, so no list
can leave a chat with a choice it can't take back. **A stored value that is no longer in the list
still applies** — the list is a suggestion for the next press, not a rule about what may already be
stored — which is also why the cap is applied on the way out and not only on the way in. The list
counts towards `enabled()`, the worker's spawn gate, since a chat can be the only reason a row is
ever written; with nothing offered, a chat can only ever keep messages, and the gate says so.

The picker has **two levels**: the groups with their current delays, then the delays for the one
that was pressed. A flat grid would need one row per group with nothing to label it by, and it
would break the moment the list grew.

Choosing a **non-zero** delay is the one press that asks Telegram anything: where the mode deletes
commands, the bot may not be allowed to, and then the answers would go while the commands stay. So
the press turns into a warning that has to be confirmed (`ONLY_WITH_COMMAND` gets a stronger one —
there nothing at all would be deleted), carrying the chosen number through the detour, and the
answer is written into the rights cache, which `ONLY_WITH_COMMAND` reads on every message
afterwards. `SelfDestructionService::may_delete_here` is that unconditional check, next to the
mode-dependent `may_delete_commands` the answering path uses. Keeping a group, or handing it back
to the bot, takes nothing away and asks nothing.

The setting is cached like the chat language and the allowed topics — the same shape, the same
lifetime, and the same read-through (see "The store of short-lived values").

**Inline messages obey the chat too, where the chat can be named.** An inline message can only be
rewritten into the placeholder, never deleted, so it gets a switch of its own in the picker rather
than following the delays quietly — the `inline` key of the same jsonb object, a boolean beside the
group names (a group can never be called that, so the two live together). It is a tri-state like the
delays: absent means the chat said nothing and `MSG_SELFDESTRUCT_INLINE_GROUPS` decides which groups
are touched, `true` means every group the chat cleans up, `false` means none.
`SelfDestructionConfig::inline_delay_for_chat` is where the two meet, and the delay is the chat's
either way.

Naming the chat is the part that isn't always possible. `inline_chosen_handler` already works only
for anchored groups (it filters on a row holding both the id and the instance), and
`inline_callback_handler` resolves one for the command itself — both simply pass what they hold.
`pvp_inline_chosen_handler` has to decode the id out of the `inline_message_id`
(`utils::try_resolve_chat_id`, under the same `CHATS_MERGING_ENABLED` its neighbours use). Where
none of that yields a chat — a legacy group without an anchor, an id that encodes none — the bot's
own settings apply, which is the half of issue #76 that `/topics` runs into as well.

Inline messages are stretched by the reading time now, the same as the others: `schedule_inline`
takes the character count from the text the handler is about to send. The battle offer is the
exception and passes zero — Telegram builds that text out of the `InlineQueryResultArticle`, so the
bot never sees it, and it is one line long anyway.

**Everything goes through the database**, short-lived groups included: `SelfDestructionService`
(`handlers/utils/self_destruction.rs`) only writes rows into `Scheduled_Message_Deletions`
(migration 37), and the worker in `scheduler/deletions.rs` claims what is due and acts on it. The
claim *leases* its batch — one `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED) RETURNING`
that pushes `fire_after` `MSG_SELFDESTRUCT_LEASE` out. The lease, not the lock, is what makes the claim
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
(see "The store of short-lived values" below), because the bot is *told* the answer rather than
having to ask for it.
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
only when a row reaches a terminal state, and the `outcome` values are those states — a rate, where
the table itself answers how many rows sit in each state right now.
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
getting in their way. A message found missing at its **warning** ends there too, rather than being
warned into the void and found missing again a grace period later — the notice is what a failed
edit costs, but a message that is gone is not coming back.
`SELECT state, count(*) … WHERE finished_at IS NOT NULL` is the first thing to look at when messages
stop disappearing, and it is a Grafana panel over the table rather than a gauge — the same trade the
broadcast queue already made. As a gauge it was published from the deletion worker's own tick, so
that query ran every `MSG_SELFDESTRUCT_POLL` — a scan of three days of finished rows, seventeen
thousand times a day, to answer what one query answers when somebody asks.
`scheduler::spawn_deletion_cleaner` deletes them `MSG_SELFDESTRUCT_TABLE_CLEANING_DELAY` days
later — a task of its own, because clearing the history must never be part of the run that wrote it.
**A retention of 0 keeps everything for ever**: right while debugging the worker, unbounded growth
on a busy bot.

The wait between attempts is **exponential** — `MSG_SELFDESTRUCT_RETRY_DELAY` doubled once
per failure already recorded, capped at `MSG_SELFDESTRUCT_MAX_RETRY_DELAY`
(`scheduler::deletions::backoff`). The count comes from the claimed row's `attempts`, not from the
`postpone` that follows: the delay has to be known before it is written. Keep the cap well under
Telegram's 48 hours. Without a cap the doubling would push an old row past that limit, and every
attempt after it is refused for sure.

**Only `MAX_AGE` stays a constant** (`scheduler::deletions`). 48 hours is Telegram's number, and a
setting for it would be a knob that changes nothing. Everything else the worker uses — the batch
size and the lease too — is an environment variable, because the right value depends on how busy
the bot is, and finding it should not need a rebuild.

**Deciding on `MSG_SELFDESTRUCT_POLL`** takes two metrics, not one, because a long tick has two
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

### The daily shrink and its broadcast queue

The shrink and the summaries it owes are **two jobs, not one** (issue #154). `run_daily_shrink`
applies the decay and writes a row per chat; `scheduler/broadcasts.rs` sends them. Nothing is held
in memory between the two.

That split is not tidiness. At ~204k chats and ~1.3M victims a day, the old single function held
the whole day's events in a `HashMap` and sent to each chat in turn — one `await` per chat, with a
user-service language call inside the loop — so a run took days rather than minutes. And the loop
around it sleeps only *after* the run returns, so a run that outlasts the day takes the next
midnight with it: five runs were logged in the fortnight before this was fixed, with the gaps
growing. Everything below follows from that.

* **The enqueue is a CTE of the shrinking statement**, so there is no moment at which a shrink is
  committed and its summary is not owed. `Scheduled_Shrink_Broadcasts` (migration 38) is a
  transactional outbox, and that is the property a message broker could not provide: publishing
  after committing is a dual write, and a crash in between loses exactly what this exists to keep.
* **The run walks the chats in batches** of `DAILY_SHRINK_BATCH_SIZE`, read from `Chats` by keyset
  on the primary key (`select_chats_batch`), so it holds one batch at a time however many chats
  there are. Per chat is the right granularity because that is the granularity a summary has: each
  batch is shrunk by one atomic statement, the locks and the memory are bounded, and a failed batch
  costs its own chats instead of the day. A `/grow` at midnight then waits behind one batch rather
  than behind every stale dick in the database.

  It reads *every* chat rather than only the ones with something to shrink, on purpose: that
  question needs a `DISTINCT` over about a million stale dicks, and it would exclude roughly one
  chat in eight, because nearly every chat has a neglected dick in it. A batch whose chats have
  nothing stale shrinks nothing and costs an index lookup.
* **Nothing comes back from the statement but counts.** The shrinks are in `Stale_Dick_Shrinks` and
  `get_shrinks_for_date` already reads exactly the page a summary needs, so the worker re-reads
  rather than carrying a payload. Page 0 therefore comes from the same `ORDER BY lost_length DESC`
  as pages 1+, which the in-memory version did not — its "next page" button could repeat or skip
  people.
* **The worker is a copy of `scheduler/deletions.rs`**: claim-with-lease, `for_each_concurrent`,
  exponential back-off, `finish()` writing the row and the counter together. `UNIQUE (chat_id,
  shrink_date)` makes the enqueue idempotent, so re-running a day can't double-send. The two
  indexes are complements — `(fire_after) WHERE finished_at IS NULL` for the claim,
  `(finished_at) WHERE finished_at IS NOT NULL` for the cleaner — each leaving out what the other
  is about.
* **`DAILY_SHRINK_BROADCAST_CONCURRENCY` is the throughput knob**, not the batch size: a run gets through
  that many messages per round trip. Keep it under `DATABASE_MAX_CONNECTIONS` — every finished
  summary writes a row — and watch `telegram_request_errors_total{kind="rate_limited"}`.
* **A rejection teloxide has a variant for is final** (`scheduler::broadcasts::is_final`): Telegram
  thought about it and refused, so the same payload gets the same answer, and three attempts across
  199k chats is an outage rather than a hiccup. `ApiError::Unknown` stays retryable — Telegram's own
  5xx answers arrive that way — unless its text says the chat is unreachable.
* **A summary older than `DAILY_SHRINK_BROADCAST_MAX_AGE` is `expired`** without spending a request.
  Yesterday's list is still news in a chat that reads once a day; last week's is noise.

```
DAILY_SHRINK_RATIO=0.01                # unset or 0 => the whole feature is off, queue included
DAILY_SHRINK_INACTIVITY_DAYS=7
DAILY_SHRINK_RAMP_UP_DAYS=7
DAILY_SHRINK_RUN_ON_STARTUP=false      # run once at startup instead of waiting for UTC midnight
DAILY_SHRINK_BATCH_SIZE=100            # chats per shrinking statement
DAILY_SHRINK_BROADCAST_POLL=5s
DAILY_SHRINK_BROADCAST_BATCH_SIZE=200  # summaries one run claims
DAILY_SHRINK_BROADCAST_CONCURRENCY=16  # how many it sends at once — the throughput knob
DAILY_SHRINK_BROADCAST_LEASE=5m
DAILY_SHRINK_BROADCAST_SEND_TIMEOUT=30s # backstop for a hang BOT_HTTP_TIMEOUT doesn't cover, see below
DAILY_SHRINK_BROADCAST_RETRY_DELAY=1m
DAILY_SHRINK_BROADCAST_MAX_RETRY_DELAY=1h
DAILY_SHRINK_BROADCAST_MAX_ATTEMPTS=3
DAILY_SHRINK_BROADCAST_MAX_AGE=48h # older than this and the summary is `expired` unsent
DAILY_SHRINK_BROADCAST_TABLE_CLEANING_DELAY=3d       # 0 => finished rows are kept for ever
```

**Whether the scheduler is alive is `daily_shrink_last_run_timestamp_seconds`**, a gauge read from
`MAX(created_at)` in `Stale_Dick_Shrinks`, not any of the `daily_shrink_*` counters. A counter that
moves once a day reads zero both when nothing happened and when nobody scraped it before the process
restarted, and nothing afterwards tells the two apart — which is how a fortnight of silence went
unnoticed. Alert on `time() - daily_shrink_last_run_timestamp_seconds > 26h`, and on
`daily_shrink_broadcast_pending` staying above zero for hours. Both live in the server-configs
repo's `vmalert/metrics-alerts.yml`, next to the Grafana dashboard.

**`daily_shrink_broadcast_last_tick_timestamp_seconds` is a faster version of the same idea, scoped
to one tick instead of one day.** It is set at the very top of the worker's loop, before `claim_due`
or a single send has run, so a tick stuck inside either one stops moving it forward within a poll
interval or two — instead of waiting for `daily_shrink_broadcast_pending` to cross the six-hour
alert threshold. It exists because of an incident where the worker froze silently for hours: a
single `send_message` never returned — not even into an error — because it hung *inside
`Throttle`'s own queue*, waiting on a lock its worker never unlocked. That wait happens before the
HTTP client is even asked to send anything, so `BOT_HTTP_TIMEOUT`/`BOT_HTTP_CONNECT_TIMEOUT` never
saw it, and the stuck tick never returned — so `ticker.tick()` was never awaited again, and the
whole worker stopped for good with no crash, no panic, and no log line to point at it.

`DAILY_SHRINK_BROADCAST_SEND_TIMEOUT` is the fix for that specific hole: a `tokio::time::timeout`
around `request.send()` (`scheduler::broadcasts::send`), bounding the send itself regardless of
*where* inside it the hang is. A timeout there ends the tick with a `Retry`, logging the fact at
`warn!` — inside the `send_and_record` span, so the log line carries the `chat_id`/`id` of whichever
summary was in flight, without having to reach for a debugger next time.

**Those three are the only gauges.** They exist because vmalert reads Prometheus and cannot query
SQL; everything a human looks at — which chats failed and why, the states over time — is a panel over
`Scheduled_Shrink_Broadcasts` through Grafana's Postgres datasource, which costs nothing when nobody
is looking. A gauge over the finished rows was the opposite trade: grouping a few hundred thousand
rows by state every five seconds so that a graph could show what one SQL query already answers.

**The heartbeat is the worker's; the two depths belong to `spawn_queue_reporter`.**
`daily_shrink_broadcast_last_tick_timestamp_seconds` has to be written by the tick it measures, so it
stays there. `daily_shrink_broadcast_pending` and `self_destruction_pending` do not: counting a queue
is a scan of its whole pending index, and hanging that off a five-second poll tied three table scans
to the workers' cadence, day and night, whether or not either queue had anything in it. They go out
every `scheduler::QUEUE_GAUGE_INTERVAL` (60s) from a task of their own — a constant on the same
grounds as `SWEEP_INTERVAL`: it decides how fresh a gauge is, not whether anything works. The
reporter runs whatever the features say, because a gauge that stops being published looks exactly
like a worker that died, which is what the alerts are watching for.

**A log line's level follows what was lost, not whether the code recovered.** A failed shrink page
is an `error!`: those chats lost the day and nothing retries it. A failed metric publication is a
`warn!`: one sample of a gauge is gone, the next tick replaces it, and the database problem behind
it has already been reported by the run that hit it.

`Chats.is_unreachable` keeps a chat out of the queue, and is cleared by `handlers::rights` on the
`my_chat_member` update that says the bot may post again — re-added, or un-muted by an
administrator. Telegram sends that update either way, so nothing polls and nobody has to type a
command in the chat first. It is only ever *taken off* there: whether the bot lost the right is what
a failed send finds out.

### One throttle for the schedulers

The daily shrink's broadcast worker and the deletion worker both reach many chats at once, and both
hold `teloxide`'s `Throttle`. They share **one**, built by `scheduler::throttled` and cloned into
both (`main.rs`).

**It only governs the broadcast.** The adaptor throttles the message-*sending* methods and passes
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
travels. A repository returns its errors instead of logging them, so by the time
`ContextLoggingErrorHandler` writes the record — in the dispatcher's task — both the repo's span
and the handler's have closed, and nothing is left to carry the ids. So the two cases part ways:

* an error **handled where it is created** — the read-through caches, the schedulers,
  `handlers::rights` — is logged inside the caller's span, and its context may be a constant;
* an error that **escapes to the dispatcher's error handler** keeps its ids in the text of its
  `.context(…)`, because nothing else will carry them there.

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

### Observing the Telegram Bot API calls

The outgoing requests are watched by `TelegramObserver` (`src/telegram_observer.rs`), attached to
the bot in `config/bot.rs`. It implements `RequestObserver`, a hook **our teloxide fork** adds to
`Bot` (branch `feature/request-observer`, `crates/teloxide-core/src/observer.rs`) — the `teloxide`
dependency in `Cargo.toml` points at that branch, and the hook is meant to be contributed upstream.

Each request produces three things:

* `telegram_request_duration_seconds{method,outcome}` — how long the call took. The failed calls
  are measured too, so a timeout (the slowest case there is) is in the histogram rather than
  missing from it. `outcome` is `ok` or one of the kinds `telegram_request_errors_total` uses.
* `telegram_request_errors_total{kind}` — the failures of that same call, from that same
  `error_handler::classify`. One classification feeds both, so the two can't disagree. It is
  counted **here** rather than at the dispatcher's error handler, which is the only place that sees
  requests at all: the schedulers never reach a dispatcher, so a broadcast-wide outage left the
  counter flat. The observer sits under every adaptor, so it sees them and polling's `getUpdates`
  alike. One consequence worth knowing: `Throttle` retries after a flood wait and the observer is
  below it, so `rate_limited` counts every rejection where the handler only ever saw the last one.
  `ContextLoggingErrorHandler` now only logs.
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

### The store of short-lived values

Everything the bot keeps briefly lives in `src/cache.rs`, which is the one place that keeps
anything: the three per-chat settings (`topics.rs`, `cleanup.rs`, the chat language in
`users/mod.rs`), the users fetched from user-service, what is known about the bot's rights in a
chat, the PVP locks (`handlers/utils/locks.rs`) and the dialogue states (`dialogue.rs`).

```
REDIS_HOST=localhost    # unset => the values are kept in this process
REDIS_PORT=6379
REDIS_PASSWORD=…
CACHE_MODE=REDIS        # REDIS | LOCAL | DISABLED; unset => REDIS with a host, LOCAL without one
```

**One store, three backends, and only one of them is a choice.** `Backend::Redis` and
`Backend::Local` hold the same keyspace — the rendered `CacheKey` — so a tenant never learns which
it got, and the fallback exists once instead of once per feature. `Local` is what an unset or
unreachable `REDIS_HOST` falls back to, so **the bot must start and run without a server**, which is
the path most likely to rot and so has a test of its own. `Disabled` is only ever asked for.

**The bot runs as a single instance, and `LOCAL` serves that perfectly well.** The issue's talk of
two instances describes what `LOCAL` could not do, not something that ever went wrong: with one
process, a map in that process is a correct cache and a correct lock. What `REDIS` buys a lone
instance is narrower and worth naming plainly — a `/promo` or `/support` dialogue that survives a
restart, and nothing else. Everything else it buys is insurance against a second instance that does
not exist yet.

The cost is on the other side of the same line: the topics gate runs on every command of every
group, and under `REDIS` each miss is a round trip where `LOCAL` had a `HashMap` lookup. Choose
accordingly, and don't read `LOCAL` as a degraded mode.

**Two tenants are not caches, and the switch doesn't reach them.** A cache may be turned off because
a miss costs a query and nothing else. A dialogue with nowhere to keep its state can never advance
past its first step, and a lock nobody holds is no lock — answering one offer twice is what the
guard is for, and not a thing to have a setting for. Both call `Cache::or_local()`, which turns a
disabled store into a local one and leaves the rest alone. So the `Local` store is always built,
whatever the mode.

That is why the locking has no switch of its own any more. `PVP_CALLBACK_LOCKS_ENABLED` was one, and
it earned its place while the lock was a `HashSet` with no expiry, where a leaked guard left a
battle unanswerable for as long as the process lived. `SET NX EX` frees itself after
`PVP_LOCK_TIME`, so the escape hatch guards nothing and only offers a way to put the bug
back. `BattleLocks` is a plain struct for the same reason: with nothing to switch and a store that
is always there, there was never a second variant to be.

**`PVP_LOCK_TIME` must outlive a handler, and that is the whole of it.** The guard frees the
lock as the handler ends, so the lifetime only bounds a process killed mid-battle. But if it runs
out while the handler is still working, the next answer goes straight through and the same attack is
resolved twice — which no release scheme can undo, since by then both are already running.

The bound is a handler's worst case: a few Telegram requests at `BOT_HTTP_TIMEOUT` plus
`pvp_impl_attack`'s four queries at `DATABASE_ACQUIRE_TIMEOUT` each. It is an estimate read
off the code, not a guarantee — nothing caps a handler's total time — so the default is 180 rather
than something snug. Generosity is cheap here: the restart after a death that leaves a lock behind
takes longer anyway.

**How long a value lives is not configured in `cache.rs`.** A lifetime belongs to the value, not to
the store: what makes a chat's language worth keeping for an hour says nothing about a lock that is
worth keeping for seconds. So every write takes one, and the numbers live together in `CachesConfig`
(`config/caches.rs`) with one variable each.

**Nothing there is a source of truth** (bar the two above). A miss is answered by the caller doing
the work again, so a server that is down, slow or simply absent costs only the work it would have
saved. Every failure is logged and swallowed: `Cache` returns no errors and never panics.

**A `Backend::Redis` that starts failing falls back for real, not just to a faster miss.** The first
failed call trips `RedisHealth::degraded` (`src/cache.rs`), and every call after that is routed by
`Backend::route` straight to `RedisState::fallback` — a local store `Backend::Redis` always carries
— without even attempting Redis. Only the map itself is built up front and free; its sweeper starts
lazily, on that same first trip, so a Redis that never fails never pays for a background task that
would spend its whole life finding an empty map. A lock taken in the fallback is a real lock, not a
`true` handed out because the store couldn't be asked; a dialogue written there is read back, not
lost until the outage ends. Trips on the *first* failure rather than after a few in a row:
`ConnectionManager` already retries internally with its own growing backoff before a call returns at
all, so waiting for several failures here would mean paying that backoff several times over, and
there is no correctness reason to wait — `RedisState::fallback` is what a single instance of the bot
already uses correctly under `CACHE_MODE=LOCAL`, not a degraded stand-in for `CACHE_MODE=REDIS`.

Recovery is a background probe, not the data path: `spawn_redis_health_check` pings Redis every
`REDIS_HEALTH_CHECK_INTERVAL` (a constant, on the same grounds as `SWEEP_INTERVAL` — it only decides
how promptly the bot notices Redis is back, not whether anything works while it waits) *only* while
degraded, and `RedisHealth::record` clears the flag on the first probe that succeeds. That same
successful probe drains `RedisState::fallback` and writes each live entry into Redis with whatever
is left of its original lifetime (`sync_fallback_to_redis`) — so a lock or a dialogue answered
mid-outage doesn't vanish the instant `Backend::route` stops reading from the fallback. It is
best-effort like everything else here: a write that fails is logged and given up on, not retried,
which for a lock is free (the same as never having been taken) and for a dialogue reads as if a
restart had just happened. Nothing keeps the fallback's copy once its write is attempted either way,
which is also what makes it empty — and its sweeper idle — the moment recovery is noticed, rather
than only once each entry's own TTL would have expired it anyway.

**`cache_fallback_active` follows the most recent call to Redis, not only the one at startup.**
`RedisHealth::record` sets it on every call that actually reaches Redis: `1` on failure, `0` on the
next success — which, once degraded, means only the health check's own probes touch it, since the
data path stops trying Redis entirely. `Cache::connect`'s own write is only the value it starts at.
So the gauge, and the alert built on it (`DickGrowerBotCacheFellBack`, `for: 5m` in server-configs),
cover a server that dies mid-session too, not only one that was never reached.

**A chat-keyed value spells the kind out** — `chat:id:-100…:topics`, not `chat:-100…:topics`. A
chat id and a chat instance are both signed 64-bit numbers, and `ChatIdKind`'s `Display` forwards to
the value, so two different chats would share an entry. `ChatIdKind::qualified` is what the keys
use, and `chat::test` pins it.

The local store expires lazily on read **and** is swept on a timer (`TASK_CACHE_SWEEPER`): a key
read once and never again would otherwise be kept for ever. One sweeper for every tenant, which is
what replaced the user-service cache's own. Its size is `cache_local_entries`, and it is the number
to watch, since it is the one that can grow without bound.

The client is `redis` with `ConnectionManager`, and there is deliberately **no pool**. Redis runs
commands one at a time, so extra connections buy no parallelism; the protocol multiplexes instead,
letting many requests share one socket, and `ConnectionManager` adds reconnection on top. A pool
would only add an acquire step that can fail, a size to choose, and a dead connection to retry.
Keep it this way unless something needs a connection to itself — `BLPOP`, `SUBSCRIBE`, `WATCH`.

**`MULTI`/`EXEC` is unsafe on a multiplexed connection; a Lua script is not.** An `EVAL` is one
command to the server and runs there without interleaving, so it is exactly as safe as any other
single command and is the way to do anything that takes more than one step. Both are in use:
`Cache::lock` is a plain `SET NX EX`, and `Cache::unlock` is a script, because "delete this key if
it still holds my token" cannot be one command and must not be two. A bare `DEL` would free
whatever holds the key — after an expiry, somebody else's lock — and that lets a third caller in
while the second is still working, which is the failure the lock exists to prevent.

The container in `docker-compose.yml` runs **Valkey**, the BSD-licensed fork. The protocol is Redis,
which is what the service, the network and the variables are named after; only the binaries differ
(`valkey-server`, `valkey-cli`). It sits behind the `redis` Compose profile, like `user-service` and
`tracing`.

**Every key begins with `KEY_PREFIX`** (`dgb:chat:id:-1001234:language`), and `Cache` applies it
rather than each key type, so a kind of value added later cannot be the one that forgets. A server
may be shared, and `chat:id:-1001234:language` says nothing about whose chat that is. The database
index in the URL isolates too; having both means neither is what everything rests on.

The user-service entry is prefixed like the rest, though the data behind it is the service's rather
than ours. Sharing it with another bot would mean agreeing on the encoding, which is a shape
invented here and read by nothing else — the store is where a saved round trip is kept, not where
two bots meet, and the service itself is that place. A user is stored as a tag byte and its encoded
message; the tag is what keeps "the service has no such user" apart from a user whose every field is
the default, which prost encodes to nothing at all.

### Dialogue state

`/promo` and `/support` have a second step, and their state used to sit in teloxide's
`InMemStorage`, so every restart dropped whatever conversation was in progress. `dialogue.rs`
implements teloxide's `Storage` over the store instead.

**Not teloxide's `RedisStorage`**, though the fork does carry the feature. That one is built on
`deadpool-redis`, which brings a second copy of the `redis` crate (0.32 beside our 1.5) and a
connection pool next to the multiplexed connection we deliberately don't pool; it keys by the bare
`chat_id`, which collides with anything else in a shared database; and it sets **no lifetime at
all**, so an abandoned dialogue would be kept for ever. The trait is three methods, so ours is
shorter than adopting that would have been, and it inherits the local fallback for free.

```
DIALOGUE_STATE_TIME=1h     # how long a half-finished command waits for its next message
```

That is not a staleness: it is how long the conversation stays open, and every answer starts it
again. `remove_dialogue` refuses when there was nothing to remove, which is what `InMemStorage` does
and what `Dialogue::exit` expects.

### Optional: user-service integration

The bot can integrate with the [user-service](https://github.com/Kozalo-Blog/user-service)
microservice (gRPC) to read/update a user's preferred language across all of Kozalo's bots:

```
GRPC_ADDR_USER_SERVICE=host:port   # unset => integration disabled, personal /language hidden in PMs
USER_CACHE_TIME=6m            # optional cache TTL for fetched users
```

`/language` is overloaded: in a private chat it changes the caller's personal language (via
user-service, above); in a group it sets a chat-wide language (admins only) that applies to
everyone and overrides each user's own preference. The chat-wide setting is stored in our own
`Chats.settings` (jsonb) column, so it works even when user-service is disabled:

```
CHAT_LANGUAGE_CACHE_TIME=1h   # optional TTL for the per-chat language cache (we own the data)
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

### Perks and the storage they are given

A perk changes a length change: `handlers/perks.rs` holds them, `perks::all` registers them, and
each is switched off by a `DISABLE_<NAME>` variable named after it. **A perk is a plugin**, so it
never gets a column of its own on `Dicks`. It gets `Perk_States` instead (migration 41) — a row per
`(chat, user, perk)` whose `state` is jsonb the perk alone understands. Nothing validates that blob:
its shape belongs to the perk, and a `CHECK` would have to name each perk's private shape to say
anything useful. `serde` is the schema, and a blob it can't read is treated as a fresh start rather
than a failure, which is what lets a perk change its stored shape without a data migration. Adding a perk therefore
needs no migration, only a row in `Perks`, and that name-to-id dictionary is read **once**, by
`Incrementor::new` at startup, in a single statement for all of them. Nothing looks a perk up by
name afterwards.

**A perk does not write.** `apply` returns its new state next to its change, and
`Dicks::create_or_grow` writes the length and every blob in one transaction. That ordering is the
whole point: the once-a-day rule is a trigger that raises `GD0E1` *after* the perks have run, and a
rolled-back growth must leave no perk believing it happened. `LoanPayoutPerk` predates this and
still pays inside `apply`, which is why a refused growth can still spend a payment; moving it here
is its own issue.

Two things a perk is handed rather than fetching itself: `ChangeSource`, because a Dick of the Day
award is not a growth and `dod_increment` shares the perk pipeline with `growth_increment`; and
`today`, which is the **database's** `current_date`, because that is the calendar the daily trigger
compares against. A perk counting days that asked this process's clock would count different ones.

The streak perk (issue #156) is the first user of all this. It stores
`{"streak": …, "max": …, "last_grow": …}` and multiplies the base increment by
`STREAK_BONUS_RATIO_PER_DAY` for each consecutive day, up to `STREAK_BONUS_MAX_DAYS`. **A shrink is
multiplied too.** Only playing on a *later* day advances the count — a second growth on the same
day, bought with `bonus_attempts`, is paid the same bonus and leaves it alone.

```
STREAK_BONUS_RATIO_PER_DAY=0.05  # of the roll, per consecutive day; 0 => the perk is off
STREAK_BONUS_MAX_DAYS=20         # days that still count, so the cap is x2; 0 => the perk is off
```

Unlike `help-pussies`, it is **on by default** (`PerksConfig::default`), so it works without a
change to server-configs.

`/stats` gets its line through `Perk::stats_line`, a hook with a default of `None`: the states are
read once and handed round, so a perk with nothing to say costs nothing.

### Dependency injection

Repositories are grouped in a `Repositories` struct and injected into handlers via the `deps!` macro. Handlers do not construct repos directly.

### Feature toggles

Runtime features are gated by environment variables parsed in `config/`. Check `config/` for the list of flags.

## Code Style

- **`new` builds a domain value; `literal!` is for the validated types only.** Five types validate
  anything: `Ratio`, `Percentage`, `FloatPercentage` (`ratio.rs`), `PromoCode` (`promo.rs`) and
  `PerkName` (`perk.rs`).
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
