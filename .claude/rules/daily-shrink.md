---
paths:
  - "src/scheduler/broadcasts.rs"
  - "src/scheduler/deletions.rs"
  - "src/scheduler/mod.rs"
  - "src/scheduler/shrink.rs"
  - "src/repo/shrinks.rs"
  - "src/repo/broadcasts.rs"
  - "src/repo/player_settings.rs"
  - "src/config/shrink.rs"
  - "src/handlers/shrink.rs"
  - "src/handlers/mentions.rs"
  - "migrations/38_create-scheduled-shrink-broadcasts-table.sql"
  - "migrations/45_broadcast-language.sql"
  - "migrations/47_analyze-stale-dick-shrinks-often.sql"
  - "migrations/51_mention-settings.sql"
---

## The daily shrink and its broadcast queue

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

  **`DAILY_SHRINK_BATCH_DELAY` is the pace of that walk**, and zero — running the batches back to
  back — is only right when the database has nothing else to do. It does: midnight is when the run
  and the chats being answered compete for the same pool. Nothing waits on this job, so resting
  between batches costs a longer run and nothing else. `200ms` over ~200k chats adds about seven
  minutes to it.
* **`Stale_Dick_Shrinks` is analyzed a hundred times more eagerly than the stock table**
  (migration 47, `autovacuum_analyze_scale_factor = 0.01`). A run writes a day's rows under a date
  the table never held, and every summary is read back by that date within minutes — so the one
  value the planner is asked about is the one its statistics know least about. A date past the end
  of the histogram is estimated at a single row, and the plan that follows is right for one row and
  ruinous for a million: a nested loop per row over `Users`, `Dicks` and `Chats`, seventeen seconds
  and eleven million buffers to answer a page of ten, sixteen of those at once until the pool is
  empty and the bot is answering nobody. With honest statistics the same query is a seek on
  `stale_dick_shrinks_idx_chat_created_at` and costs a fraction of a millisecond. The stock
  threshold — 50 rows plus a tenth of the table — is what let that happen: a night's insert is a
  smaller share of it every month the table grows, so the statistics sat days out of date. A
  hundredth keeps one night above the line. **When a night is slow again, this is the first thing to
  check**, with `last_autoanalyze` in `pg_stat_user_tables`: autovacuum decides when, not us, and
  the summaries of the first batches are claimable before it has had any reason to wake.
* **Every scheduler ticks with `MissedTickBehavior::Delay`** (`scheduler::paced`), not tokio's
  default `Burst`. A tick that overran its period would otherwise be followed at once by as many
  more as were missed, with no pause — and these loops overrun precisely when the database or
  Telegram is already struggling, which is when catching up is the one thing that makes it worse.
  Nothing is lost by it: none of them counts its ticks, each asks what is due now, so a tick that
  never happens is a tick with nothing left to do.
* **Nothing comes back from the statement but counts.** The shrinks are in `Stale_Dick_Shrinks` and
  the repository already reads exactly the page a summary needs, so the worker re-reads rather than
  carrying a payload. Page 0 therefore comes from the same `ORDER BY lost_length DESC` as pages 1+,
  which the in-memory version did not — its "next page" button could repeat or skip people.
  The worker uses `get_shrinks_for_internal_chat`, not the `get_shrinks_for_date` the chat commands
  use: `claim_due` hands it the row's own `Chats` key, so the predicate is a plain equality that
  lands on `stale_dick_shrinks_idx_chat_created_at` instead of a join to `Chats` with an `OR` across
  `chat_id` and `chat_instance`.
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
* **A summary left over from an earlier day is `expired`**, and the day is the key rather than an
  age. The run that queues a day's summaries clears the previous ones first
  (`scheduler::shrink::expire_superseded`, `shrink_date < current_date`), in batches of
  `DAILY_SHRINK_EXPIRY_BATCH_SIZE` paced by `DAILY_SHRINK_BATCH_DELAY`. Yesterday's list is still news in a chat that reads once a day — but
  not once today's is ready, because then both arrive together, and chats complained about exactly
  that. Nothing is sent to find this out: there is no age to check in `send` and no request to spend.

  **The key has to be the day, not the wall clock.** The enqueue conflicts on
  `(chat_id, shrink_date)` and does nothing for a row that is already there, finished or not. So an
  expiry that took today's rows with it — a restart with `DAILY_SHRINK_RUN_ON_STARTUP` while the
  queue is still draining, or any re-run of the day — would clear the day's broadcast and have
  nothing able to put it back. Keyed on the date, a run never touches its own rows, and re-running a
  day stays idempotent. `a_second_run_of_the_same_day_leaves_its_own_summaries_alone` pins it.

  It also means there is no knob: a summary's life is one day, because that is when the next one
  arrives.

```
DAILY_SHRINK_RATIO=0.01                # unset or 0 => the whole feature is off, queue included
DAILY_SHRINK_INACTIVITY_DAYS=7
DAILY_SHRINK_RAMP_UP_DAYS=7
DAILY_SHRINK_PERIOD_DAYS=1             # days between two runs; 7 => weekly, always on the same weekday
DAILY_SHRINK_TIME=00:00                # UTC time of day a run starts at
DAILY_SHRINK_RUN_ON_STARTUP=false      # run once at startup instead of waiting for the next start
DAILY_SHRINK_BATCH_SIZE=100            # chats per shrinking statement
DAILY_SHRINK_BATCH_DELAY=0s            # rest between two batches; 0 => back to back
DAILY_SHRINK_EXPIRY_BATCH_SIZE=5000    # rows one statement of the nightly cleanup expires
DAILY_SHRINK_BROADCAST_POLL=5s
DAILY_SHRINK_BROADCAST_BATCH_SIZE=200  # summaries one run claims
DAILY_SHRINK_BROADCAST_CONCURRENCY=16  # how many it sends at once — the throughput knob
DAILY_SHRINK_BROADCAST_LEASE=5m
DAILY_SHRINK_BROADCAST_SEND_TIMEOUT=30s # backstop for a hang BOT_HTTP_TIMEOUT doesn't cover, see below
DAILY_SHRINK_BROADCAST_RETRY_DELAY=1m
DAILY_SHRINK_BROADCAST_MAX_RETRY_DELAY=1h
DAILY_SHRINK_BROADCAST_MAX_ATTEMPTS=3
DAILY_SHRINK_BROADCAST_TABLE_CLEANING_DELAY=3d       # 0 => finished rows are kept for ever
```

**`DAILY_SHRINK_TIME` moves the run off midnight**, which is the bot's busiest hour: the batches
and the broadcast that follows them compete with the players' `/grow`s for the pool and for
Telegram's quota. It is a UTC time of day, `HH:MM`, and an unset or unreadable value is midnight.
It moves only the hour. The growth's own day still turns at UTC midnight, and every date the shrink
writes or compares (`shrink_date`, `Stale_Dick_Shrinks.created_at`, `expire_superseded`) is the
date of the run, so a start of `04:00` shrinks for the day it runs on. The liveness gauge below
reports that date *at* the start time, so its alert needs no term for it.

**`DAILY_SHRINK_PERIOD_DAYS` makes the game milder, not the cuts deeper.** The scheduler still wakes
at every start and runs only when `DailyShrinkConfig::is_due` says so — the day's number counted
from a fixed date, modulo the period, so nothing has to be remembered and a restart can't shift the
schedule. A run takes off what a daily one would; the ramp-up and the grace period are counted in
days of staleness, not in runs, so neither changes meaning. `DAILY_SHRINK_RUN_ON_STARTUP` ignores the
period. The summaries of a run stay deliverable until the next one, since `expire_superseded` only
runs when a run does.

**A player may refuse to be mentioned, and stays on the list.** The summary links each name to the
user, which is what pings them; a player who refused is printed as plain text instead. The choice is
made at two levels, and the nearer one wins: `Player_Settings` (migration 51, a jsonb row per
`(chat, uid)`, flipped by the 🔕 button under every summary) over `Users.settings` (the same key, set
with `/mentions` in a private chat — a private chat has no `Chats` row to hold it), over the default
of mentioning. The key is `shrink_mentions_enabled` in both.

It costs the broadcast nothing worth counting: a summary renders one page, and the choice arrives
in the same query — a column of the `Users` row already joined, and a `LEFT JOIN` on the primary
key of `Player_Settings` for the few rows of the page. Nothing walks a chat's members. What the
choice does *not* do is decide whether a summary is sent at all: that question would need every
victim of the chat, at the moment the shrink queues it.

The button is a toggle relative to what applies now, written in one statement
(`PlayerSettings::toggle_shrink_mentions_enabled`) — the keyboard is shared by the whole chat, so it can't
show anybody's own state, and the answer says what the press did. `Player_Settings` is carried
through a chat merge like `Perk_States` (the surviving chat wins key by key) and cleared by
`erase_user`, which also empties `Users.settings`.

**Whether the scheduler is alive is `daily_shrink_last_run_timestamp_seconds`**, a gauge read from
`MAX(created_at)` in `Stale_Dick_Shrinks` and published as that date at `DAILY_SHRINK_TIME`, not any of the `daily_shrink_*` counters. A counter that
moves once a day reads zero both when nothing happened and when nobody scraped it before the process
restarted, and nothing afterwards tells the two apart — which is how a fortnight of silence went
unnoticed. Alert on `time() - daily_shrink_last_run_timestamp_seconds > daily_shrink_period_seconds + 2h`, and on
`daily_shrink_broadcast_pending` staying above zero for hours. Both live in the server-configs
repo's `victoria-observe/vmalert/rules/metrics-alerts.yml`, next to the Grafana dashboard.

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

**A heartbeat is its own worker's; the two depths belong to `spawn_queue_reporter`.**
`daily_shrink_broadcast_last_tick_timestamp_seconds` and
`self_destruction_last_tick_timestamp_seconds` have to be written by the tick each measures, so they
stay there. That the reporter is a task of its own is exactly why neither depth can stand in for a
heartbeat: both keep being published while the worker they describe is dead, which is what the
self-destruction one was added for — that queue has no backlog alert beside it, since it is never
empty for long by design. `daily_shrink_broadcast_pending` and `self_destruction_pending` do not: counting a queue
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

