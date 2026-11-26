---
paths:
  - "src/scheduler/deletions.rs"
  - "src/scheduler/mod.rs"
  - "src/handlers/utils/self_destruction.rs"
  - "src/config/self_destruction.rs"
  - "src/cleanup.rs"
  - "src/handlers/cleanup.rs"
  - "src/domain/objects/cleanup.rs"
  - "src/repo/deletions.rs"
  - "src/telegram_backoff.rs"
  - "src/handlers/rights.rs"
  - "migrations/37_create-scheduled-message-deletions-table.sql"
  - "migrations/44_partial-index-deletions-fire-after.sql"
  - "migrations/53_deletions-removed-on-retry.sql"
---

## Optional: self-destruction of messages

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
MSG_SELFDESTRUCT_EXPIRY_BATCH_SIZE=5000 # rows one statement of the startup cleanup expires
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
lifetime, and the same read-through (see `cache.md`).

**Inline messages obey the chat too, where the chat can be named.** An inline message the bot can't
delete can only be rewritten into the placeholder, so that case gets a switch of its own in the
picker rather than following the delays quietly — the `inline` key of the same jsonb object, a
boolean beside the group names (a group can never be called that, so the two live together). It is a
tri-state like the delays: absent means the chat said nothing and `MSG_SELFDESTRUCT_INLINE_GROUPS`
decides which groups are touched, `true` means every group the chat cleans up, `false` means none.
`SelfDestructionConfig::inline_delay_for_chat` is where the two meet, and the delay is the chat's
either way.

**An inline message the bot can delete is simply deleted**, on the ordinary delays, warning
included, and neither the switch nor the list holds it back: nothing is left behind.
That is possible in a supergroup only. There, the `inline_message_id` encodes the chat and the
message's own id, which is shared by every member, so `deleteMessage` works on it as on any other
message. A legacy group's id encodes neither.

The bot also has to hold the right, and the rights cache answers that. Nothing known counts as yes
(`rights::may_delete_inline`), the same guess `ENABLED` makes for commands. `schedule_inline` picks
the delay by it (`SelfDestructionConfig::inline_delay`). The worker asks again when the row is due,
so nothing about the right is stored in the row.

A deletion that is refused teaches the cache, and the message falls back to the placeholder in the
same tick. It does so only if the switch and the list would have allowed one, read from the chat's
settings at that moment (`scheduler::deletions::Cleaner::may_shrink`); otherwise the row ends
`failed`. A message that reaches Telegram's 48 hours takes the same fallback and ends `expired`
instead.

**Every inline message is warned first, whichever way it goes**, exactly like a reply: the worker
settles on the ending — deletion, placeholder or none — before the warning, so a message that is
going to stay is never told otherwise.

Naming the chat is the part that isn't always possible. `inline_chosen_handler` already works only
for anchored groups (it filters on a row holding both the id and the instance), and
`inline_callback_handler` resolves one for the command itself — both simply pass what they hold.
`pvp_inline_chosen_handler` has to decode the id out of the `inline_message_id`
(`utils::try_resolve_chat_id`, under the same `CHATS_MERGING_ENABLED` its neighbours use). Where
none of that yields a chat — a legacy group without an anchor, an id that encodes none — the bot's
own settings apply, which is the half of issue #76 that `/topics` runs into as well.

Inline messages are stretched by the reading time now, the same as the others: `schedule_inline`
takes the character count from the text the handler is about to send. A posted battle offer comes
back without its text, so `pvp_inline_chosen_handler` builds it again from the same name, bet and
language.

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
  warning and rescheduled, rather than held in memory.

**Whether the bot may delete the command** is not in the schema at all — it lives in the cache
(see `cache.md`), because the bot is *told* the answer rather than
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

**The worker reads the cache too**, before it deletes a command. Every command a chat sends while
the answer is unknown (after the bot joins, and each time the entry expires) is queued, and they
come due together. Without the check, the first refusal taught the cache and every other one was
still sent and refused, one request each against the quota the answers need: about six a minute
in production, most of the bot's log. With it, the rest end `failed` without a request. That
refusal is the price of the guess, so it is logged at `debug`; a refusal to delete one of the
bot's own replies (`is_expected_refusal` tells them apart) still warns.

**What becomes of a row** is the whole state machine, and **no ending deletes it**:

| Ending | State |
|---|---|
| the message was deleted or replaced | `removed` |
| it was already gone (someone got there first) | `removed_before` |
| it was gone, or already replaced, after a request of ours timed out | `removed_on_retry` |
| it outlived Telegram's 48 hours while it waited | `expired` |
| the bot may not touch it, or the chat is gone | `failed` |
| every attempt failed for a reason that looked transient | `failed`, after `MAX_ATTEMPTS` |
| Telegram couldn't be reached or didn't answer in time | unchanged; postponed by `RETRY_DELAY`, `attempts` untouched |
| anything else | unchanged; postponed by the back-off, `attempts` + 1 |

**An outage spends no attempts.** A failed connection or a timeout says nothing about the message,
and a forty-minute outage — or DPI that lets connections hang — would otherwise turn every message
due during it into `failed` within minutes. The row is simply tried again every `RETRY_DELAY` until
the network is back; a long enough outage ends it as `expired` at 48 hours. A timeout is safe to
repeat here because every request the worker makes is: a second deletion finds the message gone, a
second edit finds it unchanged (`MessageNotModified`).

**A timed-out request is remembered, so that its success is not credited to somebody else.** The
row's `timed_out` flag is set by a deferral after a timeout, kept through a failed connection after
it, and cleared when the message is warned — a warning is an edit, so nothing before it can have
deleted the message. A message found gone, or a placeholder found already in place, with the flag
set ends as `removed_on_retry`: our own request most likely did it and only its answer was lost.
That keeps `removed_before` meaning someone else, and makes a run of `removed_on_retry` the sign
that the network to Telegram drops answers. A message deleted by somebody during the minute the row
waited is counted as ours; that is rare enough not to matter.

**The shrink broadcast exempts only the failed connection**, with
`DAILY_SHRINK_BROADCAST_RETRY_DELAY`, and its rows end at the next run, as `expired`. A timed-out
`sendMessage` may have been delivered, and repeating it posts the summary again; exempting it would
let a request whose answers keep getting lost post one every minute for a day. So a timeout there
still counts towards `DAILY_SHRINK_BROADCAST_MAX_ATTEMPTS`.

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

**The worker rests while Telegram is refusing us.** Every deletion and every warning is a request
to Telegram, and under a flood they compete with the answers to users for a quota that is already
spent. `telegram_backoff::TelegramBackoff` counts the `RetryAfter` refusals that `TelegramObserver`
sees (it sits under every adaptor, so it sees the refusals of the answers, the broadcast and the
worker alike).
When `MSG_SELFDESTRUCT_PAUSE_THRESHOLD` of them arrive within a minute, the worker skips its ticks
for `MSG_SELFDESTRUCT_PAUSE_FOR`, and `self_destruction_paused` reads 1.

```
MSG_SELFDESTRUCT_PAUSE_THRESHOLD=30  # refusals per minute; 0 => never pauses
MSG_SELFDESTRUCT_PAUSE_FOR=10m
```

**Only the worker pauses, never the scheduling.** A late deletion is fine, but a message that stays
for ever because it was sent during a flood is not. The rows wait with `fire_after` already past,
and the first tick after the pause claims them, bounded by the batch size and the concurrency as
always. If the catch-up provokes refusals again, the worker simply pauses again. The heartbeat is
still written on the skipped ticks, since a resting worker isn't a dead one. The threshold is well
above the occasional 429 the midnight broadcast gets, which `Throttle` retries on its own.

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

**A backlog of those is expired in bulk, once, before the worker's first tick.**
`spawn_deletion_worker` runs `ScheduledDeletions::expire_stale` in batches of
`MSG_SELFDESTRUCT_EXPIRY_BATCH_SIZE`, resting `MSG_SELFDESTRUCT_POLL` between them, each answering with a
count per group and kind that goes to the counter and to **one** `warn!`. Claimed and expired row by
row, an outage's worth of rows would cost a lease, a `finish` and a log line apiece — tens of
thousands of identical warnings for a single event, every one of them already in the table.

**Startup is the only moment such a backlog can have appeared**, which is why the run belongs there
and not on every tick. Telegram's own refusals never produce one: `MSG_SELFDESTRUCT_MAX_ATTEMPTS`
ends a row within minutes of the first failure, long before it could reach 48 hours. The only way to
accumulate them is the worker not running — a hang, a crash loop, a bot that was down — and that is
what a restart ends. Ordering it before the claim is what matters: `claim_due` orders by
`fire_after`, so the dead rows sort first and would otherwise be handed out
`MSG_SELFDESTRUCT_BATCH_SIZE` at a time, ahead of every live message. The per-row check in `act`
stays behind it at `debug`, for the row that crosses the line while the worker is up — the hour of
headroom under `MAX_AGE` allows for exactly that one.

A worker that hangs therefore also stops the only thing that clears up after it, so
`self_destruction_last_tick_timestamp_seconds` is what says it happened (see the gauges in `daily-shrink.md`).

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
* **An inline message can only be edited, unless the bot may delete it in a supergroup** (see
  above). Where it can't, it is replaced with the `self_destruction.placeholder` text instead, and
  only for the groups `MSG_SELFDESTRUCT_INLINE_GROUPS` names — whatever is put there stays in the
  chat for good, which is worth being conservative about. The bot gets an `inline_message_id` from a
  `ChosenInlineResult`, so that is where the scheduling sits. Legacy groups send no chosen result,
  so `inline_callback_handler` schedules too. The second call only fills the gap: the unique index
  on `inline_message_id` makes the insert do nothing for a message that already waits, so paging
  through a leaderboard does not delay its placeholder.

An application answered before it expires is *cancelled* (`loan.rs`, `pvp.rs`): the message has
stopped being an offer, and its outcome is kept like any other event.

