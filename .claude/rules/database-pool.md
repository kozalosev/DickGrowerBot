---
paths:
  - "src/repo/mod.rs"
  - "src/config/app.rs"
  - "src/observability.rs"
  - "src/main.rs"
---

## How long a query waits for a connection

```
DATABASE_ACQUIRE_TIMEOUT=30s  # sqlx's own default; `.env.example` suggests 5s
```

A wait here is backpressure, not patience: queueing means the pool is empty, and waiting longer
creates no connections. The default matches sqlx so that reading the variable changes nothing by
itself; the shorter value is opted into, and is what keeps a handler's worst case comfortably inside
`PVP_LOCK_TIME`. Refusals under load mean `DATABASE_MAX_CONNECTIONS` is too small, not that
this is too short.

**How long a query waited is `db_pool_acquire_seconds`.** The pool's size and its idle
connections only show that it was full, not who waited and for how long. sqlx says it in one
place only: with `acquire_time_level` set, it writes a `trace` event per acquire, target
`sqlx::pool::acquire`, carrying `acquired_after_secs`. `observability::pool_acquire_layer` reads
that event into the histogram. The layer has a filter of its own, so the event costs the console and
the export nothing. Those two names are sqlx's, not a stable API. `an_acquire_from_the_pool_is_measured`
fails if an upgrade renames them, and without it the histogram would just go flat.

An acquire that ran out the timeout returns `PoolTimedOut` and writes no event, so it is not in the
histogram. An acquire over two seconds is also logged at `warn` by sqlx itself ("acquired
connection, but time to acquire exceeded slow threshold"), which is where to look for the waits of
the time before this metric.

## How long one statement may run

The bot has no variable for this: it is Postgres's own setting, and `DATABASE_URL` already has a
place to carry those. It writes its own unit (`5s`, `500ms`), `0` means no limit, and in a URL the
space is `%20` and the equals sign `%3D`.

```
DATABASE_URL=postgres://…/dickgrowerbotdb?options=-c%20statement_timeout%3D5s
```

In server-configs that URL is built by Compose substitution, so the setting is one there too —
`…?options=-c%20statement_timeout%3D${DATABASE_STATEMENT_TIMEOUT:-0}`, with the value in `DickGrowerBot/.env`.

The pool has a limit on *waiting* and none on *running*, which is the wrong way round when a plan
goes wrong: a query with nothing to stop it holds its connection for as long as it likes, and enough
of them empty the pool while the bot answers nobody. That is a silent failure — the queries are
succeeding, slowly — where a cancelled statement is an error somebody can read. Keep it under
`DATABASE_ACQUIRE_TIMEOUT`, so a starved pool refuses at the acquire rather than after the wait.

**The migrations are exempt, and that is the one thing the code has to do about any of this.**
`repo::migrate` builds a pool of one connection, appends `statement_timeout=0` to whatever the URL
asked for — Postgres takes the last of the repeated options — runs the migrations and closes it,
before `establish_database_connection` builds the real pool. A `CREATE INDEX CONCURRENTLY` over
millions of rows is meant to take minutes and cannot lift the limit for itself: it refuses to run
inside a transaction block, which is what a multi-statement query string becomes.

A timeout is a backstop, not a fix: it makes a bad plan loud, and says nothing about why the plan is
bad. That answer is in `EXPLAIN (ANALYZE, BUFFERS)`, and usually in the statistics behind it (see
`daily-shrink.md`).

That list is only how fast a banned user sees the polite message — **the enforcement is migration
36**, a `BEFORE UPDATE` trigger on `Users` that refuses any statement touching a banned user's row
unless the statement changes `banned_until` itself (which is how the three admin functions pass).
